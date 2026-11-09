namespace Aqx.Skua;

public sealed class AqxWait(AqxBot bot, State state) : ScriptWait(new(() => bot.Flash), new(() => bot.Player), new(() => (IScriptManager)bot.Manager), new(() => bot.Map), new(() => bot.Combat), new(() => bot.Inventory), new(() => bot.Monsters), new(() => bot.House), new(() => bot.Quests), new(() => bot.Drops), new(() => bot.Skills)), IScriptWait
{
    static string Name(GameActions action) => action == GameActions.Transfer ? "tfer" : char.ToLowerInvariant(action.ToString()[0]) + action.ToString()[1..];
    public new bool IsActionAvailable(GameActions action) => IsActionAvailable(Name(action));
    public new bool IsActionAvailable(string action) => bot.Player.Playing && state.Combat.ActionLocks?.GetValueOrDefault(action) is { } timing && (timing.Used is null || DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() - (long)timing.Used >= (long)timing.Cooldown);
    public new bool ForActionCooldown(GameActions action, int timeout = 40) => ForActionCooldown(Name(action), timeout);
    public new bool ForActionCooldown(string action, int timeout = 40) => ForTrue(() => IsActionAvailable(action), OverrideTimeout ? GameActionTimeout : timeout);
}
