namespace Aqx.Skua;

public sealed class AqxFaction(State state) : IScriptFaction
{
    public List<Faction> FactionList => state.ScriptFactions;
}
