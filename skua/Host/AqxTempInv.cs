namespace Aqx.Skua;

public sealed class AqxTempInv(State state) : IScriptTempInv
{
    public List<ItemBase> Items => state.Temp.Select(i => i.ToScript()).ToList();
}
