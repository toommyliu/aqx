using Aqx.Skua;
using Skua.Core.Models;

const string Usage = """
    usage: aqx-skua check [--scripts DIR] [PATH...]
           aqx-skua run [--scripts DIR] SCRIPT
    """;

string root = ClientFileSources.SkuaScriptsDIR;
List<string> paths = [];
for (int i = 1; i < args.Length; i++)
{
    if (args[i] == "--scripts" && i + 1 < args.Length)
        root = args[++i];
    else
        paths.Add(args[i]);
}
ScriptCompiler compiler = new(Path.GetFullPath(root));

switch (args.FirstOrDefault(), paths)
{
    case ("check", []):
        return Report.Check(compiler, [compiler.Root]);
    case ("check", _):
        return Report.Check(compiler, paths);
    case ("run", [string script]):
        return Runner.Run(compiler, script);
    default:
        Console.Error.WriteLine(Usage);
        return 2;
}
