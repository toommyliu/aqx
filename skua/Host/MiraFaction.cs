namespace Mira.Skua;

public sealed class MiraFaction(State state) : IScriptFaction
{
    public List<Faction> FactionList => state.ScriptFactions;
}
