using CommunityToolkit.Mvvm.Messaging;
using Skua.Core.Messaging;

namespace Mira.Skua;

public sealed class MiraBank : IScriptBank
{
    readonly Protocol protocol;
    readonly State state;
    public MiraBank(Protocol protocol, State state)
    {
        this.protocol = protocol;
        this.state = state;
        protocol.EventReceived += data => { if (data is BankLoadedClientEvent) { Loaded = true; StrongReferenceMessenger.Default.Send(new BankLoadedMessage()); } };
        protocol.StateReceived += section => { if (section.Section == SectionName.Session && state.Session.Phase.Phase == "closed") Loaded = false; };
    }
    public int Slots => state.Me?.BankSlots ?? 0;
    public int UsedSlots => state.Me?.BankCount ?? 0;
    readonly object loadingGate = new();
    Task? loading;
    volatile bool loaded;
    public bool Loaded { get => loaded; set => loaded = value; }
    public void Open() => Load();
    public void Load(bool waitForLoad = true)
    {
        Task operation;
        lock (loadingGate)
        {
            if (Loaded) { StrongReferenceMessenger.Default.Send(new BankLoadedMessage()); return; }
            if (loading is null || loading.IsCompleted) loading = Task.Run(() => {
                try { protocol.Call(new LoadBankCall()); }
                catch (InvalidOperationException) { return; }
                Loaded = true;
                StrongReferenceMessenger.Default.Send(new BankLoadedMessage());
            });
            operation = loading;
        }
        if (waitForLoad) operation.GetAwaiter().GetResult();
    }
    public bool Swap(string invItem, string bankItem) => ((IScriptBank)this).TryGetItem(bankItem, out var bank) && IScriptInterface.Instance.Inventory.TryGetItem(invItem, out var inv) && Swap(inv!, bank!);
    public bool Swap(int invItem, int bankItem) => ((IScriptBank)this).TryGetItem(bankItem, out var bank) && IScriptInterface.Instance.Inventory.TryGetItem(invItem, out var inv) && Swap(inv!, bank!);
    public bool Swap(InventoryItem invItem, InventoryItem bankItem)
    {
        if (!IScriptInterface.Instance.Inventory.CanBank(invItem) || !((MiraInventory)IScriptInterface.Instance.Inventory).HasSpaceFor(bankItem, 1, invItem)) return false;
        try { protocol.Call(new SwapCall(invItem.ID, bankItem.ID)); }
        catch (InvalidOperationException) { return false; }
        return state.Inventory.ById.ContainsKey(bankItem.ID);
    }
    public bool EnsureSwap(string invItem, string bankItem, bool loadBank = true)
    {
        if (!IScriptInterface.Instance.Inventory.TryGetItem(invItem, out var inv) || !IScriptInterface.Instance.Inventory.CanBank(inv!)) return false;
        if (loadBank) Load();
        return ((IScriptBank)this).TryGetItem(bankItem, out var bank) && EnsureSwap(inv!, bank!);
    }
    public bool EnsureSwap(int invItem, int bankItem, bool loadBank = true)
    {
        if (!IScriptInterface.Instance.Inventory.TryGetItem(invItem, out var inv) || !IScriptInterface.Instance.Inventory.CanBank(inv!)) return false;
        if (loadBank) Load();
        return ((IScriptBank)this).TryGetItem(bankItem, out var bank) && EnsureSwap(inv!, bank!);
    }
    bool EnsureSwap(InventoryItem invItem, InventoryItem bankItem)
    {
        var bot = IScriptInterface.Instance;
        int tries = 0;
        while (!Swap(invItem, bankItem) && !bot.ShouldExit && bot.Player.Playing && ++tries < bot.Options.MaximumTries) bot.Sleep(bot.Options.ActionDelay);
        return state.Inventory.ById.ContainsKey(bankItem.ID);
    }
    public bool ToInventory(InventoryItem item) { if (!IScriptInterface.Instance.Inventory.HasSpaceFor(item)) return false; try { protocol.Call(new WithdrawCall(item.ID)); } catch (InvalidOperationException) { return false; } return state.Inventory.ById.ContainsKey(item.ID); }
    public bool EnsureToInventory(string name, bool loadBank = true) { if (loadBank) Load(); return ((IScriptBank)this).TryGetItem(name, out var item) && EnsureToInventory(item!); }
    public bool EnsureToInventory(int id, bool loadBank = true) { if (loadBank) Load(); return ((IScriptBank)this).TryGetItem(id, out var item) && EnsureToInventory(item!); }
    bool EnsureToInventory(InventoryItem item)
    {
        var bot = IScriptInterface.Instance;
        if (!bot.Inventory.HasSpaceFor(item)) return false;
        int tries = 0;
        while (!ToInventory(item) && !bot.ShouldExit && bot.Player.Playing && ++tries < bot.Options.MaximumTries) bot.Sleep(bot.Options.ActionDelay);
        return state.Inventory.ById.ContainsKey(item.ID);
    }
    public List<InventoryItem> Items => state.Bank?.ScriptItems ?? [];
}
