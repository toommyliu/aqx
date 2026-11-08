using System.Reflection;
using System.Runtime.Loader;
using CommunityToolkit.Mvvm.DependencyInjection;
using Skua.Core.Models;

namespace Mira.Skua;

public static class Runner
{
    public static MethodInfo ScriptMain(Assembly assembly) =>
        assembly.GetExportedTypes().SelectMany(t => t.GetMethods()).FirstOrDefault(m => m.Name == "ScriptMain" && m.GetParameters() is [{ ParameterType: var type }] && type == typeof(IScriptInterface)) ?? throw new InvalidOperationException("the script has no ScriptMain(IScriptInterface)");

    public static int Run(ScriptCompiler compiler, string path, IReadOnlyDictionary<string, string> options)
    {
        Stream output = Console.OpenStandardOutput();
        Console.SetOut(Console.Error);
        State state = new();
        using Protocol protocol = new(output, new StreamReader(Console.OpenStandardInput()), state);
        MiraBot bot = new(protocol, state, path) { Config = new ScriptConfig(options) };
        protocol.StopRequested += bot.RequestStop;
        protocol.Disconnected += bot.OnDisconnected;
        if (protocol.Stopping.IsCancellationRequested) bot.RequestStop();
        try
        {
            protocol.Call(new ListenCall(PacketDirection.In, false));
            if (protocol.Stopping.IsCancellationRequested) return 0;
            typeof(IScriptInterface).GetProperty(nameof(IScriptInterface.Instance))!.SetValue(null, bot);
            Ioc.Default.ConfigureServices(new Services(bot));
            string bundledSkills = new[] { Path.Combine(compiler.Root, "Skills", "AdvancedSkills.json"), Path.Combine(AppContext.BaseDirectory, "AdvancedSkills.json") }.FirstOrDefault(File.Exists) ?? "";
            string skills = ClientFileSources.SkuaAdvancedSkillsFile;
            if (bundledSkills.Length > 0 && (!File.Exists(skills) || File.GetLastWriteTimeUtc(bundledSkills) > File.GetLastWriteTimeUtc(skills) && !File.ReadAllBytes(bundledSkills).AsSpan().SequenceEqual(File.ReadAllBytes(skills))))
            {
                Directory.CreateDirectory(ClientFileSources.SkuaDIR);
                foreach (string target in new[] { skills, Path.Combine(ClientFileSources.SkuaDIR, "UserAdvancedSkills.json") })
                {
                    string temp = $"{target}.{Environment.ProcessId}";
                    File.Copy(bundledSkills, temp, true);
                    File.Move(temp, target, true);
                }
            }
            while (true)
            {
                Built built = compiler.Build(path);
                var context = new AssemblyLoadContext("script", isCollectible: true);
                var assembly = context.LoadFromStream(new MemoryStream(built.Image));
                var main = ScriptMain(assembly);
                bot.Config = new ScriptConfig(options);
                Exception? error = null;
                Thread script = new(() => {
                    try {
                        object instance = Activator.CreateInstance(main.DeclaringType!)!;
                        ((ScriptConfig)bot.Config).Attach(instance);
                        bot.ThrowIfStopped();
                        main.Invoke(instance, [bot]);
                    }
                    catch (TargetInvocationException e) { error = e.InnerException ?? e; }
                    catch (Exception e) { error = e; }
                }) { IsBackground = true, Name = "Script thread" };
                bool recover = false;
                int delay = 0;
                Task stopping = Task.CompletedTask;
                using (MiraBackground background = new(bot, protocol))
                {
                    background.Start();
                    script.Start();
                    while (!script.Join(20) && !bot.ShouldExit) { }
                    recover = bot.Disconnected && bot.Options.AutoRelogin && !protocol.Stopping.IsCancellationRequested;
                    delay = bot.Options.SafeRelogin || bot.Kicked ? 70000 : bot.Options.ReloginTryDelay;
                    if (bot.ShouldExit) script.Join(TimeSpan.FromSeconds(8));
                    if (!script.IsAlive && (bot.ShouldExit || error is not null)) stopping = bot.RaiseStopping(error is OperationCanceledException ? null : error);
                    if (recover) Await(stopping, TimeSpan.FromSeconds(2));
                }
                if (recover && !script.IsAlive && bot.PrepareRestart())
                {
                    ((MiraEvent)bot.Events).RaiseRelogin(bot.Kicked);
                    bot.Events.ClearHandlers();
                    ((MiraEvent)bot.Events).ClearFlashCalls();
                    bot.Resume();
                    try
                    {
                        Task.Delay(Math.Max(0, delay), protocol.Stopping).GetAwaiter().GetResult();
                        if (bot.Servers.EnsureRelogin(protocol.Stopping).GetAwaiter().GetResult()) { context.Unload(); continue; }
                    }
                    catch (OperationCanceledException) { }
                    bot.RequestStop();
                }
                context.Unload();
                protocol.Finish(error is not null && error is not OperationCanceledException ? EndOutcome.Failed : bot.ShouldExit ? EndOutcome.Stopped : EndOutcome.Finished, error is OperationCanceledException ? null : error?.ToString());
                Await(stopping);
                return 0;
            }
        }
        catch (OperationCanceledException) { return 0; }
        catch (Exception e) { protocol.Finish(EndOutcome.Failed, e.ToString()); return 1; }
    }
    static void Await(Task handlers, TimeSpan? timeout = null)
    {
        try { handlers.Wait(timeout ?? Timeout.InfiniteTimeSpan); } catch (AggregateException e) { Console.Error.WriteLine(e.InnerException?.Message); }
    }
    sealed class Services(MiraBot bot) : IServiceProvider
    {
        readonly IGetScriptsService scripts = new MiraGetScriptsService();
        readonly IQuestDataLoaderService quests = new global::Skua.Core.Services.QuestDataLoaderService(bot.Quests, bot.Player, bot.Flash, bot.Wait);
        readonly IDialogService dialogs = new MiraDialogService(bot);
        readonly IFileDialogService files = new MiraFileDialogService();
        readonly IThemeService themes = new MiraThemeService();
        public object? GetService(Type type) => type == typeof(IQuestDataLoaderService) ? quests : type == typeof(IGetScriptsService) ? scripts : type == typeof(IScriptInterface) ? bot : type == typeof(IScriptManager) ? bot.Manager : type == typeof(IDialogService) ? dialogs : type == typeof(IFileDialogService) ? files : type == typeof(IThemeService) ? themes : typeof(MiraBot).GetProperties().FirstOrDefault(p => p.PropertyType == type)?.GetValue(bot);
    }
}
