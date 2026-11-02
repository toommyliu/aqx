using System.Text.Json.Nodes;
using Newtonsoft.Json;
using Skua.Core.Interfaces;
using Skua.Core.Models.Auras;
using Skua.Core.Scripts;
using Skua.Core.Skills;

namespace Aqx.Skua;

public sealed partial class Shim
{
    ScriptSkill? skills;
    AdvancedSkillContainer? skillContainer;

    void Skills()
    {
        On("IScriptInterface.get_Skills()", _ => skills ??= MakeSkills());
        On("IScriptPlayer.get_Skills()", _ => null);
        On("IScriptPlayer.get_CurrentClassRank()", _ => 0);
        foreach (string owner in new[] { "IScriptSelfAuras", "IScriptTargetAuras" })
        {
            Func<List<Aura>> auras = owner == "IScriptSelfAuras" ? () => Auras(Me["auras"]) : () => Auras(Me["target"]?["auras"]);
            On($"{owner}.get_Auras()", _ => auras());
            On($"{owner}.GetAura(String)", args => Find(auras(), (string)args[0]!));
            On($"{owner}.GetAuraStacks(String)", args => (int)(Find(auras(), (string)args[0]!)?.Value ?? 0));
            On($"{owner}.GetAuraValue(String)", args => Find(auras(), (string)args[0]!)?.Value ?? 0f);
            On($"{owner}.HasActiveAura(String)", args => Find(auras(), (string)args[0]!) is not null);
            On($"{owner}.HasAnyActiveAura(String[])", args => ((string[])args[0]!).Any(name => Find(auras(), name) is not null));
        }
        On("IScriptSelfAuras.GetPlayerAura(String)", args => Find(Auras(Me["auras"]), (string)args[0]!) is { } aura ? JsonConvert.SerializeObject(aura) : null);
        On("IScriptTargetAuras.GetMonsterAura(String)", args => Find(Auras(Me["target"]?["auras"]), (string)args[0]!) is { } aura ? JsonConvert.SerializeObject(aura) : null);
    }

    ScriptSkill MakeSkills()
    {
        IScriptInterface bot = IScriptInterface.Instance;
        return new ScriptSkill(
            skillContainer ??= new AdvancedSkillContainer(),
            new(() => bot.Flash),
            new(() => bot.Player),
            new(() => bot.Self),
            new(() => bot.Target),
            new(() => bot.Combat),
            new(() => bot.Options),
            new(() => bot.Inventory),
            new(() => bot.Wait));
    }

    static List<Aura> Auras(JsonNode? auras) =>
        auras is JsonArray list
            ? list.Select(a => new Aura { Name = (string?)a?["name"] ?? "", Value = (float?)a?["stacks"] ?? 1 }).ToList()
            : [];

    static Aura? Find(List<Aura> auras, string name) => auras.Find(a => a.Name.Equals(name, StringComparison.OrdinalIgnoreCase));
}
