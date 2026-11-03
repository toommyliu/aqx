using Skua.Core.Models.Monsters;

namespace Aqx.Skua;

public sealed partial class Shim
{
    bool stopAttacking;
    bool counterHandler;

    void Combat()
    {
        On("IScriptCombat.get_StopAttacking()", _ => stopAttacking);
        On("IScriptCombat.set_StopAttacking(Boolean)", args => stopAttacking = (bool)args[0]!);
        On("IScriptCombat.get_EnableCounterHandler()", _ => counterHandler);
        On("IScriptCombat.set_EnableCounterHandler(Boolean)", args => counterHandler = (bool)args[0]!);
        On("IScriptCombat.Attack(String)", args =>
        {
            string name = (string)args[0]!;
            return Attack(m => name == "*" || m.Name.Contains(name, StringComparison.OrdinalIgnoreCase));
        });
        On("IScriptCombat.Attack(Int32)", args =>
        {
            int id = (int)args[0]!;
            return Attack(m => m.MapID == id || m.ID == id);
        });
        On("IScriptCombat.CancelTarget()", _ => Untarget());
        On("IScriptCombat.CancelAutoAttack()", _ => Untarget());
        On("IScriptCombat.Exit()", _ => ExitCombat());
        On("IScriptCombat.UntargetSelf()", _ => null);
        On("IScriptCombat.ApproachTarget()", _ => null);
        On("IScriptCombat.AttackPlayer(String)", _ => false);
    }

    bool Attack(Func<Monster, bool> matches)
    {
        if (stopAttacking)
        {
            Untarget();
            return false;
        }
        Monster? best = CellMonsters().Where(matches).OrderByDescending(m => m.HP > 0).ThenBy(m => m.HP).ThenBy(m => m.MapID).FirstOrDefault();
        if (best is null)
            return false;
        Aqx.Call("target", best.MapID);
        return true;
    }

    object? Untarget() => Aqx.Call("target", (object?)null);

    object? ExitCombat()
    {
        if (State(Me) == 1)
            return null;
        Untarget();
        return Act("exit_combat");
    }
}
