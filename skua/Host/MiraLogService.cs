namespace Mira.Skua;

public sealed class MiraLogService(Protocol protocol) : ILogService
{
    readonly Dictionary<LogType, List<string>> logs = new() { [LogType.Debug] = [], [LogType.Script] = [], [LogType.Flash] = [] };
    public void DebugLog(string message) => Write(LogType.Debug, message);
    public void ScriptLog(string message) => Write(LogType.Script, message);
    public void FlashLog(string message) => Write(LogType.Flash, message);
    void Write(LogType type, string message)
    {
        lock (logs) logs[type].Add(message);
        protocol.Call(new LogCall(LogLevel.Log, message));
    }
    public void ClearLog(LogType logType) { lock (logs) logs[logType].Clear(); }
    public List<string> GetLogs(LogType logType) { lock (logs) return [.. logs[logType]]; }
}
