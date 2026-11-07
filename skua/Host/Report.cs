using System.Reflection;
using System.Text.RegularExpressions;

namespace Mira.Skua;

public static partial class Report
{
    public static int Gaps(string root)
    {
        var missing = typeof(MiraBot).Assembly.GetTypes().Where(t => t.Name.StartsWith("Mira")).SelectMany(t => t.GetMembers().Where(m => m.GetCustomAttribute<UnsupportedAttribute>() is not null).Select(m => (t.Name, m.Name))).ToHashSet();
        var properties = typeof(IScriptInterface).GetProperties().Where(p => p.PropertyType.IsInterface).ToDictionary(p => p.Name, p => p.Name == "Config" ? nameof(ScriptConfig) : p.PropertyType.Name.StartsWith("IScript") ? "Mira" + p.PropertyType.Name[7..] : "Mira" + p.PropertyType.Name[1..]);
        foreach (var use in Directory.EnumerateFiles(root, "*.cs", SearchOption.AllDirectories).SelectMany(p => Member().Matches(File.ReadAllText(p)).Select(m => m.Value.Replace("?", "").Split('.'))).Where(parts => parts.Length == 3 ? properties.TryGetValue(parts[1], out var type) && missing.Contains((type, parts[2])) : missing.Contains(("MiraBot", parts[1]))).CountBy(p => "Bot." + string.Join('.', p.Skip(1))).OrderByDescending(g => g.Value)) Console.WriteLine($"{use.Value,6}  {use.Key}");
        return 0;
    }
    [GeneratedRegex(@"\b[Bb]ot\??\.\w+(\??\.\w+)?")]
    private static partial Regex Member();
}
