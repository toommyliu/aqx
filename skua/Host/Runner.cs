using System.Reflection;
using System.Runtime.Loader;
using Skua.Core.Interfaces;

namespace Aqx.Skua;

public static class Runner
{
    public static int Run(ScriptCompiler compiler, string script)
    {
        Outcome outcome = compiler.Build([script])[0];
        if (outcome is Failed failed)
        {
            Report.Failure(compiler, failed);
            return 1;
        }

        Built built = (Built)outcome;
        ScriptContext context = new([.. built.Closure, built]);
        Type entry = context.LoadFromAssemblyName(new AssemblyName(built.Name)).ExportedTypes.First();
        IScriptInterface bot = BotProxy.Create();
        typeof(IScriptInterface).GetProperty(nameof(IScriptInterface.Instance))!.SetValue(null, bot);

        try
        {
            object instance = Activator.CreateInstance(entry)!;
            entry.GetMethod("ScriptMain")!.Invoke(instance, [bot]);
            return 0;
        }
        catch (TargetInvocationException e) when (e.InnerException is not null)
        {
            Console.Error.WriteLine(e.InnerException);
            return 1;
        }
    }

    sealed class ScriptContext(IReadOnlyList<Built> units) : AssemblyLoadContext("script", isCollectible: true)
    {
        readonly Dictionary<string, Built> byName = units.ToDictionary(u => u.Name);

        protected override Assembly? Load(AssemblyName name) =>
            name.Name is { } n && byName.TryGetValue(n, out Built? unit) ? LoadFromStream(new MemoryStream(unit.Image)) : null;
    }
}
