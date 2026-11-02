using System.Collections.Concurrent;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.Text;

namespace Aqx.Skua;

public abstract record Outcome(string Path);

public sealed record Built(string Path, string Name, byte[] Image, IReadOnlyList<Built> Closure) : Outcome(Path)
{
    public MetadataReference Reference { get; } = MetadataReference.CreateFromImage(Image);
}

public sealed record Failed(string Path, string Cause, IReadOnlyList<string> Errors) : Outcome(Path);

public sealed record Source(string Path, string Text, IReadOnlyList<string> Includes, IReadOnlyList<string> Missing);

public sealed partial class ScriptCompiler(string root)
{
    static readonly string[] Namespaces =
    [
        "System", "System.Collections", "System.Collections.Concurrent", "System.Collections.Generic", "System.Diagnostics",
        "System.Drawing", "System.Dynamic", "System.Globalization", "System.IO", "System.Linq", "System.Net", "System.Net.Http",
        "System.Reflection", "System.Runtime.CompilerServices", "System.Text", "System.Text.RegularExpressions", "System.Threading",
        "System.Threading.Tasks", "System.Timers", "System.Windows.Forms", "Skua.Core", "Skua.Core.Interfaces", "Skua.Core.Models",
        "Skua.Core.Models.Items", "Skua.Core.Models.Monsters", "Skua.Core.Models.Players", "Skua.Core.Models.Quests",
        "Skua.Core.Models.Servers", "Skua.Core.Models.Shops", "Skua.Core.Models.Skills", "Skua.Core.Models.Auras",
        "Skua.Core.Models.Factions", "Skua.Core.Options", "Skua.Core.Utils", "CommunityToolkit.Mvvm.DependencyInjection",
        "Newtonsoft.Json", "Newtonsoft.Json.Linq",
    ];

    static readonly CSharpCompilationOptions Options = new(OutputKind.DynamicallyLinkedLibrary, optimizationLevel: OptimizationLevel.Release, deterministic: true);

    static readonly SyntaxTree GlobalUsings = CSharpSyntaxTree.ParseText(string.Concat(Namespaces.Select(n => $"global using {n};\n")), path: "<usings>");

    static readonly MetadataReference[] Framework =
        ((string)AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")!).Split(Path.PathSeparator).Select(p => MetadataReference.CreateFromFile(p)).ToArray();

    readonly Dictionary<string, string[]> listings = new();

    public string Root { get; } = root;

    public IReadOnlyList<Outcome> Build(IReadOnlyList<string> scripts)
    {
        List<string?> found = scripts.Select(Canonical).ToList();
        Dictionary<string, Outcome> outcomes = Compile(Discover(found.OfType<string>()));
        return scripts.Zip(found, (script, path) => path is null ? new Failed(script, script, [$"{script}: file not found"]) : outcomes[path]).ToList();
    }

    Dictionary<string, Outcome> Compile(Dictionary<string, Source> sources)
    {
        ConcurrentDictionary<string, Outcome> outcomes = new();
        Dictionary<string, List<string>> dependents = sources.Keys.ToDictionary(p => p, _ => new List<string>());
        Dictionary<string, int> waiting = new();
        foreach (Source source in sources.Values)
        {
            waiting[source.Path] = source.Includes.Count;
            foreach (string include in source.Includes)
                dependents[include].Add(source.Path);
        }

        List<string> wave = waiting.Where(w => w.Value == 0).Select(w => w.Key).ToList();
        while (wave.Count > 0)
        {
            Parallel.ForEach(wave, path => outcomes[path] = Compile(sources[path], outcomes));
            List<string> next = [];
            foreach (string done in wave)
                foreach (string dependent in dependents[done])
                    if (--waiting[dependent] == 0)
                        next.Add(dependent);
            wave = next;
        }

        foreach (string stuck in sources.Keys.Where(p => !outcomes.ContainsKey(p)))
            outcomes[stuck] = new Failed(stuck, stuck, [$"{Relative(stuck)}: part of an include cycle"]);
        return new Dictionary<string, Outcome>(outcomes);
    }

    public string Relative(string path) => Path.GetRelativePath(Root, path);

    string? Canonical(string path)
    {
        string full = Path.GetFullPath(path);
        string current = Path.GetPathRoot(full)!;
        foreach (string segment in full[current.Length..].Split(Path.DirectorySeparatorChar, StringSplitOptions.RemoveEmptyEntries))
        {
            if (!Directory.Exists(current))
                return null;
            if (!listings.TryGetValue(current, out string[]? entries))
                listings[current] = entries = Directory.GetFileSystemEntries(current);
            string? match = entries.FirstOrDefault(e => Path.GetFileName(e) == segment)
                ?? entries.FirstOrDefault(e => string.Equals(Path.GetFileName(e), segment, StringComparison.OrdinalIgnoreCase));
            if (match is null)
                return null;
            current = match;
        }
        return File.Exists(current) ? current : null;
    }

    Dictionary<string, Source> Discover(IEnumerable<string> roots)
    {
        Dictionary<string, Source> sources = new();
        Stack<string> pending = new(roots);
        while (pending.TryPop(out string? path))
        {
            if (sources.ContainsKey(path))
                continue;
            Source source = Read(path);
            sources[path] = source;
            foreach (string include in source.Includes)
                pending.Push(include);
        }
        return sources;
    }

    Source Read(string path)
    {
        string text = File.ReadAllText(path);
        List<string> includes = [];
        List<string> missing = [];
        foreach (string raw in text.Split('\n'))
        {
            string line = raw.Trim();
            if (!line.StartsWith("//cs_include "))
                continue;
            string include = line.Split((char[]?)null, 2, StringSplitOptions.RemoveEmptyEntries)[1];
            if ((Canonical(Path.Combine(Root, include.Replace("Scripts/", ""))) ?? Canonical(include)) is { } resolved)
                includes.Add(resolved);
            else
                missing.Add(include);
        }
        return new Source(path, text, includes.Distinct().ToList(), missing);
    }

    Outcome Compile(Source source, IReadOnlyDictionary<string, Outcome> outcomes)
    {
        List<Outcome> includes = source.Includes.Select(i => outcomes[i]).ToList();
        if (includes.OfType<Failed>().FirstOrDefault() is { } failed)
            return new Failed(source.Path, failed.Cause, failed.Errors);

        List<Built> closure = includes.Cast<Built>().SelectMany(b => b.Closure.Append(b)).DistinctBy(b => b.Path).ToList();
        string name = AssemblyName(source.Path);
        SyntaxTree tree = CSharpSyntaxTree.ParseText(SourceText.From(source.Text, Encoding.UTF8), path: Relative(source.Path));
        CSharpCompilation compilation = CSharpCompilation.Create(name, [GlobalUsings, tree], [.. Framework, .. closure.Select(b => b.Reference)], Options);

        using MemoryStream image = new();
        var result = compilation.Emit(image);
        if (result.Success)
            return new Built(source.Path, name, image.ToArray(), closure);

        List<string> errors = result.Diagnostics.Where(d => d.Severity == DiagnosticSeverity.Error).Select(d => d.ToString()).ToList();
        errors.AddRange(source.Missing.Select(m => $"{Relative(source.Path)}: include not found: {m}"));
        return new Failed(source.Path, source.Path, errors);
    }

    static string AssemblyName(string path)
    {
        string stem = Unsafe().Replace(Path.GetFileNameWithoutExtension(path), "_");
        return $"{stem}_{Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(path)))[..8]}";
    }

    [GeneratedRegex("[^A-Za-z0-9_]")]
    private static partial Regex Unsafe();
}
