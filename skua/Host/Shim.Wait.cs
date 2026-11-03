using CommunityToolkit.Mvvm.DependencyInjection;
using Skua.Core.Interfaces;
using Skua.Core.Scripts;

namespace Aqx.Skua;

public sealed partial class Shim
{
    ScriptWait? wait;
    ScriptBoost? boosts;

    void Wait()
    {
        On("IScriptInterface.get_Wait()", _ => wait ??= MakeWait());
        On("IScriptInterface.get_Boosts()", _ => boosts ??= MakeBoosts());
        On("IScriptStatus.get_ShouldExit()", _ => ShouldExit);
        On("IScriptManager.get_ShouldExit()", _ => ShouldExit);
        On("IScriptManager.get_LoadedScript()", _ => Script);
        On("IScriptStatus.get_CompiledScript()", _ => CompiledScript());
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
