namespace Mira.Skua;

public sealed class MiraTempInv(State state) : IScriptTempInv
{
    public List<ItemBase> Items => state.Temp.Select(i => i.ToScript()).ToList();
}
