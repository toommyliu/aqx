namespace Aqx.Skua;

public sealed class AqxHouseInv(Protocol protocol, State state) : IScriptHouseInv
{
    public int Slots => state.Me?.HouseSlots ?? 0;
    public int UsedSlots => Items.Count;
    public bool ToBank(InventoryItem item)
    {
        if (!IScriptInterface.Instance.Inventory.CanBank(item)) return false;
        try { protocol.Call(new DepositCall(item.ID)); }
        catch (InvalidOperationException) { return false; }
        return !Items.Any(i => i.ID == item.ID);
    }
    public bool EnsureToBank(string name) => ((IScriptHouseInv)this).TryGetItem(name, out var item) && EnsureToBank(item!);
    public bool EnsureToBank(int id) => ((IScriptHouseInv)this).TryGetItem(id, out var item) && EnsureToBank(item!);
    bool EnsureToBank(InventoryItem item)
    {
        var bot = IScriptInterface.Instance;
        if (!bot.Inventory.CanBank(item)) return false;
        int tries = 0;
        while (!ToBank(item) && !bot.ShouldExit && bot.Player.Playing && ++tries < bot.Options.MaximumTries) bot.Sleep(bot.Options.ActionDelay);
        return !Items.Any(i => i.ID == item.ID);
    }
    public List<InventoryItem> Items => state.Inventory.HouseItems;
}
