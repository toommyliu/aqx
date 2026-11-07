using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Runtime.CompilerServices;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Skua.Core.Models;

namespace Mira.Skua;

public sealed record Built(byte[] Image, string Path);
public sealed class ScriptCompiler(string root)
{
    public string Root { get; } = Path.GetFullPath(root);
    public Built Build(string path)
    {
        path = Path.GetFullPath(path);
        Dictionary<string, string> sources = new(StringComparer.OrdinalIgnoreCase);
        HashSet<string> visiting = new(StringComparer.OrdinalIgnoreCase);
        void Read(string file)
        {
            if (visiting.Contains(file)) throw new InvalidOperationException($"include cycle: {file}");
            if (sources.ContainsKey(file)) return;
            visiting.Add(file);
            string text = File.ReadAllText(file);
            sources.Add(file, text);
            foreach (string line in text.Split('\n').Select(s => s.Trim()).Where(s => s.StartsWith("//cs_include ")))
            {
                string include = line[13..].Trim().Replace('\\', '/');
                string? found = Resolve(Path.Combine(Root, include.StartsWith("Scripts/") ? include[8..] : include))
                    ?? Resolve(Path.Combine(Path.GetDirectoryName(file)!, include));
                Read(found ?? throw new FileNotFoundException($"include not found: {include}"));
            }
            visiting.Remove(file);
        }
        Read(path);
        string host = string.Join("\n", Directory.EnumerateFiles(AppContext.BaseDirectory, "*.dll").Order().Select(p => { var f = new FileInfo(p); return $"{f.Name}:{f.Length}:{f.LastWriteTimeUtc.Ticks}"; }));
        string key = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(host + string.Join("\n", sources.Select(s => s.Key + "\n" + s.Value)))));
        string cache = Path.Combine(ClientFileSources.SkuaDIR, "cache", key + ".dll");
        if (File.Exists(cache)) return new(File.ReadAllBytes(cache), path);
        byte[] image = Emit(sources, key);
        Directory.CreateDirectory(Path.GetDirectoryName(cache)!);
        string temporary = cache + "." + Guid.NewGuid() + ".tmp";
        try { File.WriteAllBytes(temporary, image); File.Move(temporary, cache, true); }
        finally { if (File.Exists(temporary)) File.Delete(temporary); }
        return new(image, path);
    }
    static string? Resolve(string path)
    {
        string full = Path.GetFullPath(path), current = Path.GetPathRoot(full)!;
        foreach (string segment in full[current.Length..].Split(Path.DirectorySeparatorChar, StringSplitOptions.RemoveEmptyEntries))
        {
            if (!Directory.Exists(current)) return null;
            current = Directory.EnumerateFileSystemEntries(current).FirstOrDefault(p => Path.GetFileName(p) == segment)
                ?? Directory.EnumerateFileSystemEntries(current).FirstOrDefault(p => string.Equals(Path.GetFileName(p), segment, StringComparison.OrdinalIgnoreCase)) ?? "";
        }
        return File.Exists(current) ? current : null;
    }
    [MethodImpl(MethodImplOptions.NoInlining)]
    static byte[] Emit(Dictionary<string, string> sources, string key)
    {
        string[] namespaces = ["System", "System.Collections", "System.Collections.Concurrent", "System.Collections.Generic", "System.Diagnostics", "System.Drawing", "System.Dynamic", "System.Globalization", "System.IO", "System.Linq", "System.Net", "System.Net.Http", "System.Reflection", "System.Text", "System.Text.RegularExpressions", "System.Threading", "System.Threading.Tasks", "System.Timers", "System.Windows.Forms", "Skua.Core", "Skua.Core.Interfaces", "Skua.Core.Models", "Skua.Core.Models.Items", "Skua.Core.Models.Monsters", "Skua.Core.Models.Players", "Skua.Core.Models.Quests", "Skua.Core.Models.Servers", "Skua.Core.Models.Shops", "Skua.Core.Models.Skills", "Skua.Core.Models.Auras", "Skua.Core.Models.Factions", "Skua.Core.Options", "Skua.Core.Utils", "CommunityToolkit.Mvvm.DependencyInjection", "Newtonsoft.Json", "Newtonsoft.Json.Linq"];
        SyntaxTree usings = CSharpSyntaxTree.ParseText(string.Concat(namespaces.Select(n => $"global using {n};\n")));
        string[] assemblies = ((string)AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")!).Split(Path.PathSeparator);
        CSharpCompilation compilation = CSharpCompilation.Create("Script_" + key, [usings, .. sources.Select(s => CSharpSyntaxTree.ParseText(s.Value, path: s.Key))], assemblies.Select(p => MetadataReference.CreateFromFile(p)), new(OutputKind.DynamicallyLinkedLibrary, optimizationLevel: OptimizationLevel.Release, deterministic: true));
        using MemoryStream image = new();
        var result = compilation.Emit(image);
        if (!result.Success) throw new InvalidOperationException(string.Join("\n", result.Diagnostics.Where(d => d.Severity == DiagnosticSeverity.Error)));
        return image.ToArray();
    }
}
