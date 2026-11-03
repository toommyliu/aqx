using System.Collections.Concurrent;
using System.Diagnostics.CodeAnalysis;
using System.Reflection;
using System.Text.Json.Nodes;
using Skua.Core.Interfaces;
using Skua.Core.Models;

namespace Aqx.Skua;

public delegate object? Handler(object?[] args);

public delegate object? TypedHandler(object?[] args, Type returns);

public sealed partial class Shim
{
    public const string ScriptThread = "Script Thread";

    static readonly ConcurrentDictionary<(Type Owner, MethodInfo Method), string> Keys = new();

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

    readonly Dictionary<string, Func<object?[], MethodInfo, object?>> handlers = [];
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
        Player();
        Map();
        Monsters();
        Combat();
        Items();
        Handlers();
        Events();
        Send();
        Services();
        Servers();
        Ultra();
        Flash();
        Wait();
        Drops();
        Accounts();
        Skills();
        Quests();
        Shops();
        Validate();
    }

    public AqxClient Aqx { get; }
    public ScriptConfig Config { get; }
    public string Script { get; }
    public bool StopRequested => stopRequested;
    public bool ShouldExit => stopRequested || Aqx.Stopping.IsCancellationRequested;

    public bool TryGet(Type owner, MethodInfo method, [MaybeNullWhen(false)] out Func<object?[], MethodInfo, object?> handler) =>
        handlers.TryGetValue(Key(owner, method), out handler);

    public static string Key(Type owner, MethodInfo method) =>
        Keys.GetOrAdd((owner, method), k =>
        {
            MethodInfo m = k.Method.IsGenericMethod ? k.Method.GetGenericMethodDefinition() : k.Method;
            string arity = m.IsGenericMethodDefinition ? $"`{m.GetGenericArguments().Length}" : "";
            return $"{k.Owner.Name}.{m.Name}{arity}({string.Join(", ", m.GetParameters().Select(p => p.ParameterType.Name))})";
        });

    void On(string key, Handler handler) => handlers.Add(key, (args, _) => handler(args));

    void On(string key, TypedHandler handler) => handlers.Add(key, (args, method) => handler(args, method.ReturnType));

    void Infrastructure()
    {
        On("IScriptInterface.get_Version()", _ => null);
        On("IScriptInterface.get_ShouldExit()", _ => ShouldExit);
        On("IScriptInterface.Sleep(Int32)", args =>
        {
            Sleep((int)args[0]!);
            return null;
        });
        On("IScriptInterface.Log(String)", args => Aqx.Call("log", "info", args[0] ?? ""));
        On("IScriptInterface.Stop(Boolean)", args =>
        {
            Stop((bool)args[0]!);
            return null;
        });
        On("IScriptInterface.StopAsync(Boolean)", args =>
        {
            Stop((bool)args[0]!);
            return Task.CompletedTask;
        });
        On("IScriptInterface.get_Random()", _ => random);
        On("IScriptInterface.set_Random(Random)", args => random = (Random)args[0]!);
        On("IScriptInterface.get_Config()", _ => Config);
        On("IScriptInterface.ShowMessageBox(String, String, Boolean)", args => Dismiss(args, null));
        On("IScriptInterface.ShowMessageBox(String, String, String[])", args => Dismiss(args, DialogResult.Cancelled));
        On("IScriptStatus.get_LoadedScript()", _ => Script);
    }

    JsonNode? View(string name) => Aqx.Call("view", name);

    JsonObject Me => Aqx.Call("view", "player") as JsonObject ?? [];

    string Phase => (string?)Aqx.Call("session")?["phase"] ?? "";

    bool Act(string call, params object?[] args)
    {
        try
        {
            Aqx.Call(call, args);
            return true;
        }
        catch (AqxException e) when (e.Code != "cancelled")
        {
            Aqx.Call("log", "warn", $"{call} failed: {e.Message}");
            return false;
        }
    }

    void Background(string name, Action loop) =>
        new Thread(() =>
        {
            try
            {
                loop();
            }
            catch (AqxException e) when (e.Code == "cancelled")
            {
            }
        }) { IsBackground = true, Name = name }.Start();

    void Stop(bool runScriptStoppingEvent)
    {
        runStopping = runScriptStoppingEvent;
        stopRequested = true;
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
                On(Key(type, get), _ => settings.GetValueOrDefault(name, fallback));
            if (property.SetMethod is { } set)
                On(Key(type, set), args =>
                {
                    settings[name] = args[0];
                    if (name == "IScriptOption.AggroMonsters")
                        Aqx.Call("aggro", args[0]);
                    return null;
                });
        }
    }

    void Validate()
    {
        HashSet<string> members = typeof(IScriptInterface).Assembly.GetTypes()
            .Where(t => t.IsInterface && !t.IsGenericTypeDefinition)
            .SelectMany(t => t.GetInterfaces().Prepend(t).SelectMany(i => i.GetMethods(BindingFlags.Instance | BindingFlags.Public | BindingFlags.DeclaredOnly)).Select(m => Key(t, m)))
            .ToHashSet();
        string[] unknown = handlers.Keys.Where(k => !members.Contains(k)).ToArray();
        if (unknown.Length > 0)
            throw new InvalidOperationException($"handlers for members that don't exist: {string.Join(", ", unknown)}");
    }
}
