using Skua.Core.Models;

namespace Aqx.Skua;

public static class Serve
{
    public static int Run(ScriptCompiler compiler, string script, IReadOnlyDictionary<string, string> overrides)
    {
        Directory.CreateDirectory(ClientFileSources.SkuaOptionsDIR);
        Stream protocol = Console.OpenStandardOutput();
        Console.SetOut(Console.Error);
        AqxClient aqx = new(protocol, new StreamReader(Console.OpenStandardInput()));

        Outcome compiled = compiler.Build([script])[0];
        if (compiled is Failed failed)
        {
            aqx.Finish("failed", Report.Describe(compiler, failed));
            return 1;
        }

        string skills = Path.Combine(compiler.Root, "Skills", "AdvancedSkills.json");
        if (File.Exists(skills) && !File.Exists(ClientFileSources.SkuaAdvancedSkillsFile))
            File.Copy(skills, ClientFileSources.SkuaAdvancedSkillsFile);
        Shim shim = new(aqx, new ScriptConfig(overrides), Path.GetFullPath(script));
        shim.Start();
        (string Outcome, string? Detail) end = ("finished", null);
        Thread thread = new(() => end = Execute((Built)compiled, shim)) { Name = Shim.ScriptThread };
        thread.Start();
        thread.Join();
        if (!aqx.Stopping.IsCancellationRequested)
            aqx.Finish(end.Outcome, end.Detail);
        return 0;
    }

    static (string, string?) Execute(Built built, Shim shim)
    {
        try
        {
            Runner.Start(built, shim);
            shim.ScriptStopping(null);
            return (shim.StopRequested ? "stopped" : "finished", null);
        }
        catch (Exception e) when (shim.ShouldExit && e is OperationCanceledException or AqxException { Code: "cancelled" })
        {
            shim.ScriptStopping(null);
            return ("stopped", null);
        }
        catch (Exception e)
        {
            shim.ScriptStopping(e);
            return ("failed", e.ToString());
        }
    }
}
