using Newtonsoft.Json;
namespace Mira.Skua;

public sealed class MiraTargetAuras(State state) : IScriptTargetAuras
{
    AurasSnapshot? Current => ((MiraCombat)IScriptInterface.Instance.Combat).SelectedTarget is { Kind: "monster" } target ? state.Room?.MonstersById.GetValueOrDefault(target.Id)?.Auras : null;
    public List<Aura> Auras => Current?.Active.Concat(Current.Passive).Select(MiraAuras.ToScript).ToList() ?? [];
    public List<AuraSnapshot> Snapshots => Current?.Active.Select(MiraAuras.Snapshot).ToList() ?? [];
    public AuraSnapshot? GetAuraSnapshot(string auraName) => Snapshots.FirstOrDefault(a => a.Name.Equals(auraName, StringComparison.OrdinalIgnoreCase));
    public int GetAuraStacks(string auraName) => GetAuraSnapshot(auraName)?.Stacks ?? 0;
    public float GetAuraRuleStacks(string auraName) => Math.Max(GetAuraStacks(auraName), Auras.Where(a => a.Name.Equals(auraName, StringComparison.OrdinalIgnoreCase)).Sum(a => a.Value));
    public bool HasActiveAura(string auraName) => GetAura(auraName) is not null;
    public Aura? GetAura(string auraName) => Auras.FirstOrDefault(a => a.Name.Equals(auraName, StringComparison.OrdinalIgnoreCase));
    public float GetAuraValue(string auraName) => GetAura(auraName)?.Value ?? 444;
    public bool HasAnyActiveAura(params string[] auraNames) => auraNames.Any(HasActiveAura);
    public string GetMonsterAura(string monsterName) => Serialize(state.Room?.Monsters.LastOrDefault(m => m.Name.Equals(monsterName, StringComparison.OrdinalIgnoreCase))?.Auras);
    public string GetMonsterAura(int monID) => Serialize(state.Room?.MonstersById.GetValueOrDefault(monID)?.Auras);
    static string Serialize(AurasSnapshot? auras) => auras is null ? "Error: Couldn't get Monster Object Tree" : JsonConvert.SerializeObject(auras.Active.Concat(auras.Passive).Select(MiraAuras.ToScript));
}
