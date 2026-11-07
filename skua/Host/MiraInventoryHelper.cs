namespace Mira.Skua;

public sealed class MiraInventoryHelper() : ScriptInventoryHelper(new(() => IScriptInterface.Instance.Inventory), new(() => IScriptInterface.Instance.House), new(() => IScriptInterface.Instance.TempInv), new(() => IScriptInterface.Instance.Bank));
