namespace Aqx.Skua;

public sealed class AqxInventoryHelper() : ScriptInventoryHelper(new(() => IScriptInterface.Instance.Inventory), new(() => IScriptInterface.Instance.House), new(() => IScriptInterface.Instance.TempInv), new(() => IScriptInterface.Instance.Bank));
