using CommunityToolkit.Mvvm.Messaging;
using Skua.Core.Messaging;
using System.Text.Json;

namespace Mira.Skua;

public sealed class MiraShop : IScriptShop
{
    readonly Protocol protocol;
    readonly State state;
    public MiraShop(Protocol protocol, State state)
    {
        this.protocol = protocol;
        this.state = state;
        protocol.StateReceived += section => {
            if (section.Section != SectionName.Shop || section.Value.ValueKind == JsonValueKind.Null) return;
            var shop = section.Value.Deserialize(StateJson.Default.ShopSnapshot)!;
            var info = new ShopInfo((int)shop.Id, shop.Name, shop.ScriptItems);
            if (!LoadedCache.Contains(info)) LoadedCache.Add(info);
            StrongReferenceMessenger.Default.Send<ShopLoadedMessage, int>(new(info), (int)MessageChannels.GameEvents);
        };
    }
    public bool IsLoaded => state.Shop is not null;
    public int ID => (int)(state.Shop?.Id ?? 0);
    public List<ShopItem> Items => state.Shop?.ScriptItems ?? [];
    public List<ShopInfo> LoadedCache { get; set; } = [];
    public string Name => state.Shop?.Name ?? "";
    public bool ProtectFavorites { get; set; } = true;
    public List<string> GetUnmetPurchaseRequirements(ShopItem item, int quantity = -1)
    {
        var listing = state.Shop?.Items.FirstOrDefault(i => i.Id == item.ID && i.Listing == item.ShopItemID);
        if (listing is null) return ["Item is not in the loaded shop."];
        int purchased = quantity == -1 ? Math.Max(1, listing.Qty) : quantity;
        if (purchased <= 0) return ["Purchase quantity must be positive."];
        List<string> missing = [];
        if (listing.QuestSlot >= 0 && IScriptInterface.Instance.Flash.CallGameFunction<int>("world.getQuestValue", (int)listing.QuestSlot) < listing.QuestValue) missing.Add("Quest requirement not met.");
        int bundles = Math.Max(1, purchased / Math.Max(1, listing.Qty));
        Dictionary<long, (long Quantity, string Name)> required = [];
        foreach (var need in listing.Merge)
        {
            required.TryGetValue(need.Id, out var previous);
            required[need.Id] = (previous.Quantity + (long)need.Qty * bundles, need.Name);
        }
        if (listing.Token > 0 && listing.TokenQty > 0)
        {
            required.TryGetValue(listing.Token, out var previous);
            required[listing.Token] = (previous.Quantity + (long)listing.TokenQty * bundles, string.IsNullOrEmpty(listing.TokenName) ? previous.Name : listing.TokenName);
        }
        foreach (var (id, need) in required)
        {
            var held = state.Inventory.ById.GetValueOrDefault(id);
            int have = held?.Qty ?? 0;
            if (have < need.Quantity) missing.Add($"Missing {need.Quantity - have} x {(need.Name ?? held?.Name ?? $"Item #{id}")} in inventory.");
        }
        return missing;
    }
    public void BuyItem(string name, int quantity = -1) => Buy(Items.Find(i => i.Name.Equals(name, StringComparison.OrdinalIgnoreCase)), quantity);
    public void BuyItem(int id, int shopItemId = -1, int quantity = -1) => Buy(Items.Find(i => i.ID == id && (shopItemId == -1 || i.ShopItemID == shopItemId)), quantity);
    void Buy(ShopItem? item, int quantity)
    {
        if (item is null || quantity != -1 && quantity <= 0 || GetUnmetPurchaseRequirements(item, quantity).Count > 0) return;
        int count = quantity == -1 ? Math.Max(1, item.Quantity) : quantity;
        if (!IScriptInterface.Instance.Inventory.HasSpaceFor(item, count)) return;
        try { protocol.Call(new BuyCall(item.ShopItemID, (uint)count)); }
        catch (InvalidOperationException) { }
    }
    public void EnsureLoadAndBuyItem(int shopId, string itemName, int quantity = -1) { Load(shopId); BuyItem(itemName, quantity); }
    public void EnsureLoadAndBuyItem(int shopId, int itemId, int shopItemId = -1, int quantity = -1) { Load(shopId); BuyItem(itemId, shopItemId, quantity); }
    public void Load(int id)
    {
        try { protocol.Call(new LoadShopCall(id)); }
        catch (InvalidOperationException) { }
    }
    [Unsupported] public void LoadArmourCustomizer() => throw new NotSupportedException("IScriptShop.LoadArmourCustomizer");
    [Unsupported] public void LoadHairShop(int id) => throw new NotSupportedException("IScriptShop.LoadHairShop");
    public void SellItem(string name, int quantity = -1) => Sell(state.Inventory.InventoryItems.Find(i => i.Name.Equals(name, StringComparison.OrdinalIgnoreCase)), quantity);
    public void SellItem(int id, int quantity = -1) => Sell(state.Inventory.InventoryItems.Find(i => i.ID == id), quantity);
    void Sell(InventoryItem? item, int quantity)
    {
        if (item is null || ProtectFavorites && IScriptInterface.Instance.Inventory.IsFavorited(item.ID)) return;
        int count = quantity == -1 ? item.Quantity : Math.Min(quantity, item.Quantity);
        if (count <= 0) return;
        try { protocol.Call(new SellCall(item.ID, (uint)count)); }
        catch (InvalidOperationException) { }
    }
    public int MaximumBuys(ShopItem item, bool merge)
    {
        if (item.ItemGroup == "ar") return 1;
        int room = item.MaxStack - (state.Inventory.ById.GetValueOrDefault(item.ID)?.Qty ?? 0);
        if (room < 1) return 0;
        double most = room;
        if (item.Cost > 0) most = Math.Min(most, Math.Floor((double)(item.Coins ? state.Me?.Coins ?? 0 : state.Me?.Gold ?? 0) / item.Cost));
        if (merge)
        {
            foreach (var need in item.Requirements)
            {
                if (!state.Inventory.ById.TryGetValue(need.ID, out var held)) return 0;
                int sets = held.Qty / Math.Max(1, need.Quantity);
                if (sets == 0) return 0;
                most = Math.Min(most, sets);
            }
            most *= item.Quantity;
        }
        if (item.TokenID > 0 && item.TokenQuantity > 0) most = Math.Min(most, Math.Floor((state.Inventory.ById.GetValueOrDefault(item.TokenID)?.Qty ?? 0) / item.TokenQuantity) * item.Quantity);
        return (int)Math.Min(Math.Min(most, 100000), room);
    }
}
