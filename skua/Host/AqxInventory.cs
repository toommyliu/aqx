namespace Aqx.Skua;

public sealed class AqxInventory(Protocol protocol, State state) : IScriptInventory
{
    public int Slots => state.Me?.BagSlots ?? 0;
    public int UsedSlots => state.Inventory.BagUsed;
    public bool ToBank(InventoryItem item) { if (!CanBank(item)) return false; try { protocol.Call(new DepositCall(item.ID)); } catch (InvalidOperationException) { return false; } return !state.Inventory.ById.ContainsKey(item.ID); }
    public bool EnsureToBank(string name) => ((IScriptInventory)this).TryGetItem(name, out var item) && EnsureToBank(item!);
    public bool EnsureToBank(int id) => ((IScriptInventory)this).TryGetItem(id, out var item) && EnsureToBank(item!);
    bool EnsureToBank(InventoryItem item)
    {
        var bot = IScriptInterface.Instance;
        if (!CanBank(item)) return false;
        int tries = 0;
        while (!ToBank(item) && !bot.ShouldExit && bot.Player.Playing && ++tries < bot.Options.MaximumTries) bot.Sleep(bot.Options.ActionDelay);
        return !state.Inventory.ById.ContainsKey(item.ID);
    }
    public void EquipItem(int id) { try { protocol.Call(new EquipUncheckedCall(id)); } catch (InvalidOperationException) { } }
    public void EquipUsableItem(InventoryItem? item) { if (item is not null && item.CategoryString.Equals("item", StringComparison.OrdinalIgnoreCase)) EquipItem(item.ID); }
    public bool HasCategories => true;
    public int MiscSlots => Slots;
    public int MiscUsedSlots => state.Inventory.MiscUsed;
    public int MiscFreeSlots => MiscSlots - MiscUsedSlots;
    public string GetPool(ItemBase item) => ItemSnapshot.Classify(item.CategoryString, item.Meta ?? "", item.House).ToString().ToLowerInvariant();
    public bool HasSpaceFor(ItemBase item, int quantity = 1) => HasSpaceFor(item, quantity, null);
    public bool HasSpaceFor(ItemBase item, int quantity, ItemBase? outgoing)
    {
        var held = state.Inventory.ById.GetValueOrDefault(item.ID);
        var pool = ItemSnapshot.Classify(item.CategoryString, item.Meta ?? "", item.House);
        if (pool == InventoryPool.Class) return true;
        if (pool == InventoryPool.House) return state.Inventory.HouseItems.Count < (state.Me?.HouseSlots ?? 0);
        if (held is not null && held.Qty < held.MaxStack) return true;
        int freed = outgoing is not null && GetPool(outgoing) == GetPool(item) ? 1 : 0;
        if (held is null && item is ShopItem shop)
        {
            int bundles = Math.Max(1, quantity / Math.Max(1, shop.Quantity));
            bool tokenMerged = false;
            foreach (var need in shop.Requirements)
            {
                double count = (double)need.Quantity * bundles;
                if (shop.TokenID > 0 && shop.TokenQuantity > 0 && need.ID == shop.TokenID) { count += shop.TokenQuantity * bundles; tokenMerged = true; }
                if (need.ID != 45739 && state.Inventory.ById.GetValueOrDefault(need.ID) is { } input && input.Pool == pool && (input.Slot == "ar" || input.Qty <= count)) freed++;
            }
            if (!tokenMerged && shop.TokenID > 0 && shop.TokenQuantity > 0 && shop.TokenID != 45739 && state.Inventory.ById.GetValueOrDefault(shop.TokenID) is { } token && token.Pool == pool && token.Qty <= shop.TokenQuantity * bundles) freed++;
        }
        return pool == InventoryPool.Misc ? held is not null || MiscUsedSlots - freed < MiscSlots : UsedSlots - freed < Slots;
    }
    public bool CanBank(InventoryItem item) => item.Category != ItemCategory.Class;
    public bool IsFavorited(int itemId) => IScriptInterface.Instance.Flash.Call<bool>("isFavoriteItem", itemId);
    public List<InventoryItem> Items => state.Inventory.InventoryItems;
}
