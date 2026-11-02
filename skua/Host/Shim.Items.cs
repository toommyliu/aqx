using CommunityToolkit.Mvvm.Messaging;
using Skua.Core.Messaging;
using Skua.Core.Models.Items;

namespace Aqx.Skua;

public sealed partial class Shim
{
    void Items()
    {
        On("IScriptInventory.get_Items()", _ => InventoryItems());
        On("IScriptInventory.get_Slots()", _ => Int(View("slots")!["bag"]));
        On("IScriptInventory.get_UsedSlots()", _ => Int(View("slots")!["bagUsed"]));
        On("IScriptInventory.EquipItem(Int32)", args => Act("equip", args[0]));
        On("IScriptInventory.EquipUsableItem(InventoryItem)", args => Act("equip", ((InventoryItem)args[0]!).ID));
        On("IScriptInventory.ToBank(InventoryItem)", args => Act("deposit", ((InventoryItem)args[0]!).ID));
        On("IScriptInventory.EnsureToBank(String)", args => Act("deposit", args[0]));
        On("IScriptInventory.EnsureToBank(Int32)", args => Act("deposit", args[0]));

        On("IScriptTempInv.get_Items()", _ => Models.Items<ItemBase>(View("temp"), temp: true));

        On("IScriptHouseInv.get_Items()", _ => new List<InventoryItem>());
        On("IScriptHouseInv.get_Slots()", _ => 0);
        On("IScriptHouseInv.get_UsedSlots()", _ => 0);

        On("IScriptBank.get_Items()", _ => Models.Items<InventoryItem>(View("bank")));
        On("IScriptBank.get_Slots()", _ => Int(View("slots")!["bank"]));
        On("IScriptBank.get_UsedSlots()", _ => Int(View("slots")!["bankUsed"]));
        On("IScriptBank.get_Loaded()", _ => View("bank") is not null);
        On("IScriptBank.set_Loaded(Boolean)", _ => null);
        On("IScriptBank.Load(Boolean)", _ => LoadBank());
        On("IScriptBank.Open()", _ => LoadBank());
        On("IScriptBank.ToInventory(InventoryItem)", args => Act("withdraw", ((InventoryItem)args[0]!).ID));
        On("IScriptBank.EnsureToInventory(String, Boolean)", args => Act("withdraw", args[0]));
        On("IScriptBank.EnsureToInventory(Int32, Boolean)", args => Act("withdraw", args[0]));
        On("IScriptBank.Swap(String, String)", args => Act("deposit", args[0]) && Act("withdraw", args[1]));
        On("IScriptBank.Swap(Int32, Int32)", args => Act("deposit", args[0]) && Act("withdraw", args[1]));
        On("IScriptBank.Swap(InventoryItem, InventoryItem)", args => Act("deposit", ((InventoryItem)args[0]!).ID) && Act("withdraw", ((InventoryItem)args[1]!).ID));
        On("IScriptBank.EnsureSwap(String, String, Boolean)", args => Act("deposit", args[0]) && Act("withdraw", args[1]));
        On("IScriptBank.EnsureSwap(Int32, Int32, Boolean)", args => Act("deposit", args[0]) && Act("withdraw", args[1]));
    }

    bool LoadBank()
    {
        if (!Act("load_bank"))
            return false;
        StrongReferenceMessenger.Default.Send(new BankLoadedMessage());
        return true;
    }
}
