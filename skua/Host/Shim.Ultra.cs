using System.Text.RegularExpressions;
using Skua.Core.Interfaces;
using Skua.Core.Models.Auras;

namespace Aqx.Skua;

public sealed partial class Shim
{
    bool counterEnabled;
    bool counterActive;
    int? targetBeforeCounter;

    void Ultra()
    {
        On("IUltraBossHelper.EnableCounterAttack()", _ =>
        {
            counterEnabled = counterHandler = true;
            return null;
        });
        On("IUltraBossHelper.DisableCounterAttack()", _ =>
        {
            counterEnabled = counterActive = counterHandler = false;
            return null;
        });
        On("IUltraBossHelper.get_IsCounterAttackEnabled()", _ => counterEnabled);
        On("IUltraBossHelper.get_IsCounterAttackActive()", _ => counterActive);
        On("IUltraBossHelper.SetAttacksStopped(Boolean)", args =>
        {
            SetAttacksStopped((bool)args[0]!);
            return null;
        });
        On("IUltraBossHelper.AnalyzeChargeMechanics(IScriptSelfAuras, String, String, String)", args =>
        {
            IScriptSelfAuras auras = (IScriptSelfAuras)args[0]!;
            string positive = (string)args[1]!, negative = (string)args[2]!;
            string? reversed = (string?)args[3];
            bool hasReversed = !string.IsNullOrEmpty(reversed) && auras.HasAnyActiveAura(positive + reversed, negative + reversed);
            return (auras.HasActiveAura(positive), auras.HasActiveAura(negative), hasReversed);
        });
        On("IUltraBossHelper.CheckAuraThreshold(IScriptSelfAuras, String, Int32, String)", args =>
        {
            float value = ((IScriptSelfAuras)args[0]!).GetAuraValue((string)args[1]!);
            int threshold = (int)args[2]!;
            return (string?)args[3] switch
            {
                "<" => value < threshold,
                ">" => value > threshold,
                "<=" => value <= threshold,
                ">=" => value >= threshold,
                "==" => value == threshold,
                "!=" => value != threshold,
                var other => throw new ArgumentException($"Invalid comparison operator: {other}"),
            };
        });
        On("IUltraBossHelper.GetAurasMatchingPattern(List`1, String)", args =>
        {
            List<Aura> auras = (List<Aura>)args[0]!;
            string pattern = (string)args[1]!;
            if (!pattern.Contains('*'))
                return auras.Where(a => a.Name.Equals(pattern, StringComparison.OrdinalIgnoreCase)).ToList();
            Regex regex = new("^" + pattern.Replace("*", ".*") + "$", RegexOptions.IgnoreCase);
            return auras.Where(a => regex.IsMatch(a.Name)).ToList();
        });
        On("IUltraBossHelper.ShouldUseSkill(IScriptSelfAuras, Dictionary`2)", args =>
            ((Dictionary<string, Func<float, bool>>)args[1]!).All(c => c.Value(((IScriptSelfAuras)args[0]!).GetAuraValue(c.Key))));
        On("IUltraBossHelper.ShouldUseSkill(IScriptTargetAuras, Dictionary`2)", args =>
            ((Dictionary<string, Func<float, bool>>)args[1]!).All(c => c.Value(((IScriptTargetAuras)args[0]!).GetAuraValue(c.Key))));
    }

    void SetAttacksStopped(bool stopped)
    {
        stopAttacking = stopped;
        if (stopped)
            return;
        counterActive = false;
        targetBeforeCounter = null;
    }

    void Countering(bool active)
    {
        if (!counterHandler)
            return;
        if (active)
        {
            counterActive = true;
            targetBeforeCounter = Me["target"] is { } target ? Int(target["mapId"]) : null;
            stopAttacking = true;
            Untarget();
            return;
        }
        int? previous = targetBeforeCounter;
        SetAttacksStopped(false);
        if (previous is { } id)
            Aqx.Call("target", id);
    }
}
