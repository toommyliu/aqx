using System.Diagnostics;
using System.Reflection;
using System.Text.RegularExpressions;
using Skua.Core.Interfaces;

namespace Aqx.Skua;

public static partial class Report
{
    const int ErrorsShown = 5;

    public static int Check(ScriptCompiler compiler, IEnumerable<string> targets)
    {
        Stopwatch clock = Stopwatch.StartNew();
        List<string> scripts = targets.SelectMany(Expand).Where(IsScript).Distinct().ToList();
        List<Failed> failed = compiler.Build(scripts).OfType<Failed>().ToList();

        Console.WriteLine($"compiled {scripts.Count - failed.Count} of {scripts.Count} scripts in {clock.Elapsed.TotalSeconds:F0} s");
        foreach (IGrouping<string, Failed> cause in failed.GroupBy(f => f.Cause).OrderByDescending(g => g.Count()))
        {
            Console.WriteLine();
            int blocked = cause.Count(f => f.Path != cause.Key);
            Console.WriteLine(blocked > 0 ? $"{compiler.Relative(cause.Key)} fails, blocking {blocked} scripts" : $"{compiler.Relative(cause.Key)} fails");
            Errors(cause.First());
        }
        return failed.Count == 0 ? 0 : 1;
    }

    public static int Gaps(ScriptCompiler compiler)
    {
        Shim shim = new(new AqxClient(Stream.Null, TextReader.Null), new ScriptConfig(new Dictionary<string, string>()), "");
        Dictionary<string, int> uses = Directory.EnumerateFiles(compiler.Root, "*.cs", SearchOption.AllDirectories)
            .SelectMany(path => BotMember().Matches(File.ReadAllText(path)).Select(m => "Bot" + m.Value[3..].Replace("?", "")))
            .CountBy(member => member)
            .ToDictionary();
        foreach ((string member, int count) in uses.Where(u => Missing(shim, u.Key)).OrderByDescending(u => u.Value))
            Console.WriteLine($"{count,6}  {member}");
        return 0;
    }

    static bool Missing(Shim shim, string use)
    {
        string[] parts = use.Split('.');
        if (typeof(IScriptInterface).GetProperty(parts[1]) is { PropertyType.IsInterface: true } group)
            return parts.Length == 3 && !shim.TryGet(typeof(IScriptInterface), group.GetMethod!, out _) && Unhandled(shim, group.PropertyType, parts[2]);
        return Unhandled(shim, typeof(IScriptInterface), parts[1]);
    }

    static bool Unhandled(Shim shim, Type owner, string name)
    {
        MethodInfo[] methods = owner.GetInterfaces().Prepend(owner)
            .SelectMany(t => t.GetMethods())
            .Where(m => m.Name == name || m.Name.EndsWith($"_{name}"))
            .ToArray();
        return methods.Length > 0 && !methods.Any(m => !m.IsAbstract || shim.TryGet(owner, m, out _));
    }

    [GeneratedRegex(@"\b[Bb]ot\??\.\w+(\??\.\w+)?")]
    private static partial Regex BotMember();

    public static string Describe(ScriptCompiler compiler, Failed failed)
    {
        string what = failed.Cause == failed.Path
            ? $"{compiler.Relative(failed.Path)} doesn't compile"
            : $"{compiler.Relative(failed.Path)} includes {compiler.Relative(failed.Cause)}, which doesn't compile";
        return string.Join("\n", failed.Errors.Prepend(what));
    }

    static void Errors(Failed failed)
    {
        foreach (string error in failed.Errors.Take(ErrorsShown))
            Console.WriteLine($"  {error}");
        if (failed.Errors.Count > ErrorsShown)
            Console.WriteLine($"  and {failed.Errors.Count - ErrorsShown} more errors");
    }

    static IEnumerable<string> Expand(string target) =>
        Directory.Exists(target) ? Directory.EnumerateFiles(target, "*.cs", SearchOption.AllDirectories).Select(Path.GetFullPath) : [Path.GetFullPath(target)];

    static bool IsScript(string path) => File.ReadAllText(path).Contains("void ScriptMain(");
}
