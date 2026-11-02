using System.Collections.Concurrent;
using System.Diagnostics.CodeAnalysis;
using System.Reflection;
using Skua.Core.Interfaces;
using Skua.Core.Models;

namespace Aqx.Skua;

public delegate object? Handler(object?[] args);

public sealed class Shim
{
    public const string ScriptThread = "Script Thread";

    static readonly ConcurrentDictionary<MethodInfo, string> Keys = new();

    static readonly Dictionary<string, object> SkuaDefaults = new()
    {
        ["IScriptOption.SafeTimings"] = true,
        ["IScriptOption.RetryRelogin"] = true,
        ["IScriptOption.LoadTimeout"] = 30000,
        ["IScriptOption.HuntDelay"] = 1000,
        ["IScriptOption.HuntBuffer"] = 1,
        ["IScriptOption.MaximumTries"] = 10,
        ["IScriptOption.ActionDelay"] = 800,
        ["IScriptOption.JoinMapTries"] = 3,
        ["IScriptOption.QuestAcceptAndCompleteTries"] = 30,
        ["IScriptOption.ReloginTries"] = 5,
        ["IScriptOption.ReloginTryDelay"] = 800,
        ["IScriptOption.LoginTimeout"] = 30000,
    };

    readonly Dictionary<string, Handler> handlers = [];
    readonly ConcurrentDictionary<string, object?> settings = new();
    Random random = new();
    volatile bool stopRequested;

    public Shim(AqxClient aqx, ScriptConfig config, string script)
    {
        Aqx = aqx;
        Config = config;
        Script = script;
        Infrastructure();
        Settings(typeof(IScriptOption));
        Settings(typeof(IScriptLite));
        Validate();
    }

    public AqxClient Aqx { get; }
    public ScriptConfig Config { get; }
    public string Script { get; }
    public bool StopRequested => stopRequested;
    public bool ShouldExit => stopRequested || Aqx.Stopping.IsCancellationRequested;

    public bool TryGet(MethodInfo method, [MaybeNullWhen(false)] out Handler handler) => handlers.TryGetValue(Key(method), out handler);

    public static string Key(MethodInfo method) =>
        Keys.GetOrAdd(method, m => $"{m.DeclaringType!.Name}.{m.Name}({string.Join(", ", m.GetParameters().Select(p => p.ParameterType.Name))})");

    void On(string key, Handler handler) => handlers.Add(key, handler);

    void Infrastructure()
    {
        On("IScriptInterface.get_Version()", _ => typeof(IScriptInterface).Assembly.GetName().Version);
        On("IScriptInterface.get_ShouldExit()", _ => ShouldExit);
        On("IScriptInterface.Sleep(Int32)", args =>
        {
            Sleep((int)args[0]!);
            return null;
        });
        On("IScriptInterface.Log(String)", args => Aqx.Call("log", "info", args[0] ?? ""));
        On("IScriptInterface.Stop(Boolean)", _ =>
        {
            stopRequested = true;
            return null;
        });
        On("IScriptInterface.StopAsync(Boolean)", _ =>
        {
            stopRequested = true;
            return Task.CompletedTask;
        });
        On("IScriptInterface.get_Random()", _ => random);
        On("IScriptInterface.set_Random(Random)", args => random = (Random)args[0]!);
        On("IScriptInterface.get_Config()", _ => Config);
        On("IScriptInterface.ShowMessageBox(String, String, Boolean)", args => Dismiss(args, null));
        On("IScriptInterface.ShowMessageBox(String, String, String[])", args => Dismiss(args, DialogResult.Cancelled));
        On("IScriptStatus.get_LoadedScript()", _ => Script);
    }

    public void Sleep(int ms)
    {
        if (ShouldExit && Thread.CurrentThread.Name == ScriptThread)
            throw new OperationCanceledException();
        Thread.Sleep(ms);
    }

    object? Dismiss(object?[] args, object? answer)
    {
        Aqx.Call("log", "info", $"{args[1]}: {args[0]} (dismissed, as if the window were closed)");
        return answer;
    }

    void Settings(Type type)
    {
        foreach (PropertyInfo property in type.GetProperties())
        {
            string name = $"{type.Name}.{property.Name}";
            object? fallback = SkuaDefaults.GetValueOrDefault(name) ?? (property.PropertyType.IsValueType ? Activator.CreateInstance(property.PropertyType) : null);
            if (property.GetMethod is { } get)
                On(Key(get), _ => settings.GetValueOrDefault(name, fallback));
            if (property.SetMethod is { } set)
                On(Key(set), args => settings[name] = args[0]);
        }
    }

    void Validate()
    {
        HashSet<string> members = typeof(IScriptInterface).Assembly.GetTypes()
            .Where(t => t.IsInterface)
            .SelectMany(t => t.GetMethods(BindingFlags.Instance | BindingFlags.Public | BindingFlags.DeclaredOnly))
            .Select(Key)
            .ToHashSet();
        string[] unknown = handlers.Keys.Where(k => !members.Contains(k)).ToArray();
        if (unknown.Length > 0)
            throw new InvalidOperationException($"handlers for members that don't exist: {string.Join(", ", unknown)}");
    }
}
