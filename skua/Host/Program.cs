using Mira.Skua;
using Skua.Core.Models;

try
{
    string? root = null;
    List<string> paths = [];
    Dictionary<string, string> options = [];
    Dictionary<string, string> changes = [];
    for (int i = 1; i < args.Length; i++)
    {
        if (args[i] == "--scripts" && i + 1 < args.Length) root = args[++i];
        else if (args[i] == "--arg" && i + 1 < args.Length && args[++i].Split('=', 2) is [string name, string value]) options[name] = value;
        else if (args[i] == "--set" && i + 1 < args.Length && args[++i].Split('=', 2) is [string key, string set]) changes[key] = set;
        else paths.Add(args[i]);
    }
    if (args.FirstOrDefault() == "fixtures" && paths is [string fixtures]) return ProtocolFixtures.Check(fixtures);
    if (root is null && paths.Count == 1)
        for (DirectoryInfo? dir = new FileInfo(paths[0]).Directory; dir is not null; dir = dir.Parent)
            if (File.Exists(Path.Combine(dir.FullName, "CoreBots.cs"))) { root = dir.FullName; break; }
    ScriptCompiler compiler = new(root ?? ClientFileSources.SkuaScriptsDIR);
    switch (args.FirstOrDefault(), paths)
    {
        case ("check", [string path]): compiler.Build(path); return 0;
        case ("serve", [string path]): return Runner.Run(compiler, path, options);
        case ("options", [string path]): return Settings.ScriptOptions(compiler, path, changes);
        case ("cbo", [string user]): return Settings.CoreBots(user, changes);
        case ("gaps", []): return Report.Gaps(compiler.Root);
        default: Console.Error.WriteLine("usage: mira-skua check|serve SCRIPT [--scripts DIR] [--arg Name=value], options SCRIPT [--set Group:Name=value], cbo USER [--set Key=value], or gaps --scripts DIR"); return 2;
    }
}
catch (Exception e) { Console.Error.WriteLine(e.Message); return 1; }
