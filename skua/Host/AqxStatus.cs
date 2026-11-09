namespace Aqx.Skua;

public sealed class AqxStatus(AqxBot bot, string path) : IScriptManager
{
    public bool ShouldExit => bot.ShouldExit;
    public bool ScriptRunning => !ShouldExit;
    public string LoadedScript { get; private set; } = path;
    public string CompiledScript => string.Join(Environment.NewLine, File.ReadLines(LoadedScript).Where(line => !line.TrimStart().StartsWith("//cs_"))).Trim();
    public IScriptOptionContainer? Config { get => bot.Config; set => throw new NotSupportedException("IScriptStatus.Config"); }
    public CancellationTokenSource? ScriptCts => bot.ScriptCts;
    public Task RestartScriptAsync() => Task.CompletedTask;
    public void SetLoadedScript(string path) => LoadedScript = path;
    public ValueTask StopScript(bool runScriptStoppingEvent = true) { bot.Stop(runScriptStoppingEvent); return ValueTask.CompletedTask; }
    [Unsupported] public Task<Exception?> StartScript() => throw new NotSupportedException("IScriptManager.StartScript");
    [Unsupported] public object? Compile(string source) => throw new NotSupportedException("IScriptManager.Compile");
    [Unsupported] public void LoadScriptConfig(object? script) => throw new NotSupportedException("IScriptManager.LoadScriptConfig");
    public event PropertyChangedEventHandler? PropertyChanged;
}
