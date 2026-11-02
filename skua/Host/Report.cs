using System.Diagnostics;

namespace Aqx.Skua;

public static class Report
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
