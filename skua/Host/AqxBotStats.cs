namespace Aqx.Skua;

public sealed class AqxBotStats : IScriptBotStats
{
    [Unsupported] public int Deaths { get => throw new NotSupportedException("IScriptBotStats.Deaths"); set => throw new NotSupportedException("IScriptBotStats.Deaths"); }
    [Unsupported] public int Drops { get => throw new NotSupportedException("IScriptBotStats.Drops"); set => throw new NotSupportedException("IScriptBotStats.Drops"); }
    [Unsupported] public int Kills { get => throw new NotSupportedException("IScriptBotStats.Kills"); set => throw new NotSupportedException("IScriptBotStats.Kills"); }
    [Unsupported] public int QuestsAccepted { get => throw new NotSupportedException("IScriptBotStats.QuestsAccepted"); set => throw new NotSupportedException("IScriptBotStats.QuestsAccepted"); }
    [Unsupported] public int QuestsCompleted { get => throw new NotSupportedException("IScriptBotStats.QuestsCompleted"); set => throw new NotSupportedException("IScriptBotStats.QuestsCompleted"); }
    [Unsupported] public int Relogins { get => throw new NotSupportedException("IScriptBotStats.Relogins"); set => throw new NotSupportedException("IScriptBotStats.Relogins"); }
    [Unsupported] public int InventorySpace { get => throw new NotSupportedException("IScriptBotStats.InventorySpace"); }
    [Unsupported] public int InventoryFilledSpace { get => throw new NotSupportedException("IScriptBotStats.InventoryFilledSpace"); }
    [Unsupported] public int InventoryFreeSpace { get => throw new NotSupportedException("IScriptBotStats.InventoryFreeSpace"); }
    [Unsupported] public int BankSpace { get => throw new NotSupportedException("IScriptBotStats.BankSpace"); }
    [Unsupported] public int BankFilledSpace { get => throw new NotSupportedException("IScriptBotStats.BankFilledSpace"); }
    [Unsupported] public int BankFreeSpace { get => throw new NotSupportedException("IScriptBotStats.BankFreeSpace"); }
    [Unsupported] public void Reset()  => throw new NotSupportedException("IScriptBotStats.Reset");
    [Unsupported] public void GetSpace()  => throw new NotSupportedException("IScriptBotStats.GetSpace");
    [Unsupported] public event System.ComponentModel.PropertyChangedEventHandler? PropertyChanged { add => throw new NotSupportedException("AqxBotStats.PropertyChanged"); remove => throw new NotSupportedException("AqxBotStats.PropertyChanged"); }
}
