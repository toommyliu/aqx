using System.Text.Json.Nodes;
using CommunityToolkit.Mvvm.Messaging;
using Skua.Core.Messaging;
using Skua.Core.Models.Items;
using Skua.Core.Models.Shops;

namespace Aqx.Skua;

public sealed partial class Shim
{
    void Shops()
    {
        On("IScriptShop.get_IsLoaded()", _ => ShopInfo() is not null);
        On("IScriptShop.get_ID()", _ => Int(ShopInfo()?["ShopID"]));
        On("IScriptShop.get_Name()", _ => (string?)ShopInfo()?["sName"] ?? "");
        On("IScriptShop.get_Items()", _ => ShopItems());
        On("IScriptShop.Load(Int32)", args => Act("load_shop", args[0]));
        On("IScriptShop.BuyItem(String, Int32)", args =>
            Buy(i => i.Name.Equals((string)args[0]!, StringComparison.OrdinalIgnoreCase), (int)args[1]!));
        On("IScriptShop.BuyItem(Int32, Int32, Int32)", args =>
            Buy(i => i.ID == (int)args[0]! && ((int)args[1]! == -1 || i.ShopItemID == (int)args[1]!), (int)args[2]!));
        On("IScriptShop.SellItem(String, Int32)", args =>
            Sell(InventoryItems().Find(i => i.Name.Equals((string)args[0]!, StringComparison.OrdinalIgnoreCase)), (int)args[1]!));
        On("IScriptShop.SellItem(Int32, Int32)", args => Sell(InventoryItems().Find(i => i.ID == (int)args[0]!), (int)args[1]!));
    }

    JsonObject? ShopInfo() => Aqx.Call("shopinfo") as JsonObject;

    List<ShopItem> ShopItems() =>
        ShopInfo()?["items"] is JsonArray items ? Newtonsoft.Json.JsonConvert.DeserializeObject<List<ShopItem>>(items.ToJsonString()) ?? [] : [];

    bool Buy(Predicate<ShopItem> match, int quantity) =>
        ShopItems().Find(match) is { } item && Buy(item.ID, item.ShopItemID, quantity < 0 ? Math.Max(1, item.Quantity) : quantity);

    bool Buy(int item, int listing, int quantity)
    {
        if (!Act("buy", Int(ShopInfo()?["ShopID"]), item, listing, quantity))
            return false;
        StrongReferenceMessenger.Default.Send(new ItemBoughtMessage(0));
        return true;
    }

    bool Sell(InventoryItem? item, int quantity)
    {
        if (item is null)
            return false;
        int sold = quantity < 0 ? item.Quantity : Math.Min(quantity, item.Quantity);
        if (!Act("sell", item.ID, sold))
            return false;
        StrongReferenceMessenger.Default.Send(new ItemSoldMessage(item.CharItemID, sold, item.Quantity - sold, 0, item.Coins));
        return true;
    }
}
