using CommunityToolkit.Mvvm.DependencyInjection;
using Skua.Core.Interfaces;
using Skua.Core.Scripts;

namespace Aqx.Skua;

public sealed partial class Shim
{
    ScriptWait? wait;
    ScriptBoost? boosts;
    ScriptKill? kill;
    ScriptHunt? hunt;

    void Wait()
    {
        On("IScriptInterface.get_Wait()", _ => wait ??= MakeWait());
        On("IScriptInterface.get_Boosts()", _ => boosts ??= MakeBoosts());
        On("IScriptInterface.get_Kill()", _ => kill ??= MakeKill());
        On("IScriptInterface.get_Hunt()", _ => hunt ??= MakeHunt());
        On("IScriptStatus.get_ShouldExit()", _ => ShouldExit);
        On("IScriptManager.get_ShouldExit()", _ => ShouldExit);
        On("IScriptManager.get_LoadedScript()", _ => Script);
        On("IScriptStatus.get_CompiledScript()", _ => CompiledScript());
        On("IScriptStatus.RestartScriptAsync()", _ => Task.CompletedTask);
        On("IScriptManager.RestartScriptAsync()", _ => Task.CompletedTask);
    }

    string CompiledScript() =>
        string.Join(Environment.NewLine, File.ReadAllLines(Script).Where(line =>
            !(line.TrimStart().StartsWith("//cs_") && line.Split((char[]?)null, 2, StringSplitOptions.RemoveEmptyEntries).Length == 2))).Trim();

    static ScriptBoost MakeBoosts()
    {
        IScriptInterface bot = IScriptInterface.Instance;
        return new ScriptBoost(
            new(() => bot.Flash),
            new(() => bot.Send),
            new(() => bot.Map),
            new(() => bot.Inventory),
            new(() => bot.Bank),
            new(() => bot.Player),
            new(() => bot.Wait));
    }

    static ScriptKill MakeKill()
    {
        IScriptInterface bot = IScriptInterface.Instance;
        return new ScriptKill(
            new(() => bot.Wait),
            new(() => bot.Options),
            new(() => bot.Combat),
            new(() => bot.Player),
            new(() => bot.Drops),
            new(() => bot.Inventory),
            new(() => bot.TempInv),
            new(() => Ioc.Default.GetRequiredService<IScriptManager>()),
            new(() => bot.Map),
            Ioc.Default.GetRequiredService<ILogService>(),
            new(() => bot.Monsters));
    }

    static ScriptHunt MakeHunt()
    {
        IScriptInterface bot = IScriptInterface.Instance;
        return new ScriptHunt(
            new(() => bot.Options),
            new(() => bot.Combat),
            new(() => bot.Player),
            new(() => bot.Monsters),
            new(() => bot.Drops),
            new(() => bot.Inventory),
            new(() => bot.TempInv),
            new(() => Ioc.Default.GetRequiredService<IScriptManager>()),
            new(() => bot.Map),
            new(() => bot.Kill),
            Ioc.Default.GetRequiredService<ILogService>());
    }

    static ScriptWait MakeWait()
    {
        IScriptInterface bot = IScriptInterface.Instance;
        IScriptManager manager = Ioc.Default.GetRequiredService<IScriptManager>();
        return new ScriptWait(
            new(() => bot.Flash),
            new(() => bot.Player),
            new(() => manager),
            new(() => bot.Map),
            new(() => bot.Combat),
            new(() => bot.Inventory),
            new(() => bot.Monsters),
            new(() => bot.House),
            new(() => bot.Quests),
            new(() => bot.Drops),
            new(() => bot.Skills));
    }
}
