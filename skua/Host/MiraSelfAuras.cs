using Newtonsoft.Json;
namespace Mira.Skua;

public sealed class MiraSelfAuras(State state) : IScriptSelfAuras
{
    AurasSnapshot? Current => state.Me?.Auras;
    public List<Aura> Auras => Current?.Active.Concat(Current.Passive).Select(MiraAuras.ToScript).ToList() ?? [];
    public List<AuraSnapshot> Snapshots => Current?.Active.Select(MiraAuras.Snapshot).ToList() ?? [];
    public AuraSnapshot? GetAuraSnapshot(string auraName) => Snapshots.FirstOrDefault(a => a.Name.Equals(auraName, StringComparison.OrdinalIgnoreCase));
    public int GetAuraStacks(string auraName) => GetAuraSnapshot(auraName)?.Stacks ?? 0;
    public float GetAuraRuleStacks(string auraName) => Math.Max(GetAuraStacks(auraName), Auras.Where(a => a.Name.Equals(auraName, StringComparison.OrdinalIgnoreCase)).Sum(a => a.Value));
    public bool HasActiveAura(string auraName) => GetAura(auraName) is not null;
    public Aura? GetAura(string auraName) => Auras.FirstOrDefault(a => a.Name.Equals(auraName, StringComparison.OrdinalIgnoreCase));
    public float GetAuraValue(string auraName) => GetAura(auraName)?.Value ?? 444;
    public bool HasAnyActiveAura(params string[] auraNames) => auraNames.Any(HasActiveAura);
    public string GetPlayerAura(string playerName) => JsonConvert.SerializeObject(state.Me?.Name.Equals(playerName, StringComparison.OrdinalIgnoreCase) == true ? Current!.Active.Concat(Current.Passive).Select(MiraAuras.ToScript) : state.Room?.Players.FirstOrDefault(p => p.Name.Equals(playerName, StringComparison.OrdinalIgnoreCase))?.Auras is { } auras ? auras.Active.Concat(auras.Passive).Select(MiraAuras.ToScript) : []);
}
