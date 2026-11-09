global using ScriptOption = Aqx.Skua.AqxOption;

namespace Aqx.Skua;

public sealed class AqxDrop : ScriptDrop
{
    readonly HashSet<int> rejectedOffers = [];
    public AqxDrop(Protocol protocol, State state) : base(new(() => IScriptInterface.Instance.Flash), new(() => IScriptInterface.Instance.Send), new(() => IScriptInterface.Instance.Map), new(() => IScriptInterface.Instance.Wait), new(() => IScriptInterface.Instance.Options), new(() => IScriptInterface.Instance.Player), new(() => IScriptInterface.Instance.Inventory))
    {
        protocol.EventReceived += data => {
            if (data is DropOfferedClientEvent offered) lock (rejectedOffers) rejectedOffers.Remove((int)offered.Id);
        };
    }
    public bool RejectOffers(IEnumerable<string> keep)
    {
        HashSet<string> whitelist = new(keep, StringComparer.OrdinalIgnoreCase);
        lock (rejectedOffers) foreach (var drop in CurrentDropInfos) if (!whitelist.Contains(drop.Name)) rejectedOffers.Add(drop.ID);
        return true;
    }
    public IEnumerable<ItemBase> VisibleOffers { get { lock (rejectedOffers) return CurrentDropInfos.Where(i => !rejectedOffers.Contains(i.ID)).ToArray(); } }
}
