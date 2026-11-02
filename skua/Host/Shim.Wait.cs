using CommunityToolkit.Mvvm.DependencyInjection;
using Skua.Core.Interfaces;
using Skua.Core.Scripts;

namespace Aqx.Skua;

public sealed partial class Shim
{
    ScriptWait? wait;

    void Wait()
    {
        On("IScriptInterface.get_Wait()", _ => wait ??= MakeWait());
        On("IScriptStatus.get_ShouldExit()", _ => ShouldExit);
        On("IScriptManager.get_ShouldExit()", _ => ShouldExit);
        On("IScriptManager.get_LoadedScript()", _ => Script);
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
