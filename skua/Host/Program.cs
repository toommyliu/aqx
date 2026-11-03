using Aqx.Skua;
using Skua.Core.Models;

const string Usage = """
    usage: aqx-skua check [--scripts DIR] [PATH...]
           aqx-skua gaps [--scripts DIR]
           aqx-skua serve [--scripts DIR] SCRIPT [--arg KEY=VALUE]...
    """;

string? root = null;
List<string> paths = [];
Dictionary<string, string> overrides = [];
for (int i = 1; i < args.Length; i++)
{
    if (args[i] == "--scripts" && i + 1 < args.Length)
        root = args[++i];
    else if (args[i] == "--arg" && i + 1 < args.Length && args[++i].Split('=', 2) is [string key, string value])
        overrides[key] = value;
    else
        paths.Add(args[i]);
}

switch (args.FirstOrDefault(), paths)
{
    case ("check", []):
        return Report.Check(Compiler(root), [Compiler(root).Root]);
    case ("check", _):
        return Report.Check(Compiler(root), paths);
    case ("gaps", []):
        return Report.Gaps(Compiler(root));
    case ("serve", [string script]):
        return Serve.Run(Compiler(root ?? ScriptsAbove(script)), script, overrides);
    default:
        Console.Error.WriteLine(Usage);
        return 2;
}

static ScriptCompiler Compiler(string? root) => new(Path.GetFullPath(root ?? ClientFileSources.SkuaScriptsDIR));

static string? ScriptsAbove(string script)
{
    for (DirectoryInfo? dir = new FileInfo(script).Directory; dir is not null; dir = dir.Parent)
        if (File.Exists(Path.Combine(dir.FullName, "CoreBots.cs")))
            return dir.FullName;
    return null;
}
