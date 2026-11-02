using System.Reflection;
using System.Runtime.ExceptionServices;
using System.Runtime.Loader;
using Skua.Core.Interfaces;

namespace Aqx.Skua;

public static class Runner
{
    public static void Start(Built built, Shim shim)
    {
        ScriptContext context = new([.. built.Closure, built]);
        Type entry = context.LoadFromAssemblyName(new AssemblyName(built.Name)).ExportedTypes.First();
        MethodInfo main = entry.GetMethod("ScriptMain") ?? throw new InvalidOperationException($"{entry.Name} has no ScriptMain");
        IScriptInterface bot = BotProxy.Create(shim);
        typeof(IScriptInterface).GetProperty(nameof(IScriptInterface.Instance))!.SetValue(null, bot);
        Unwrapped(() =>
        {
            object instance = Activator.CreateInstance(entry)!;
            shim.Config.Attach(instance);
            main.Invoke(instance, [bot]);
        });
    }

    static void Unwrapped(Action action)
    {
        try
        {
            action();
        }
        catch (TargetInvocationException e) when (e.InnerException is not null)
        {
            ExceptionDispatchInfo.Capture(e.InnerException).Throw();
        }
    }

    sealed class ScriptContext(IReadOnlyList<Built> units) : AssemblyLoadContext("script", isCollectible: true)
    {
        readonly Dictionary<string, Built> byName = units.ToDictionary(u => u.Name);

        protected override Assembly? Load(AssemblyName name) =>
            name.Name is { } n && byName.TryGetValue(n, out Built? unit) ? LoadFromStream(new MemoryStream(unit.Image)) : null;
    }
}
