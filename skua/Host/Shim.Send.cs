using Skua.Core.Models;

namespace Aqx.Skua;

public sealed partial class Shim
{
    void Send()
    {
        On("IScriptSend.Packet(String, String)", args =>
        {
            Remember((string)args[0]!);
            return Act("send", args[0]);
        });
        On("IScriptSend.ClientPacket(String, String)", _ => null);
        foreach (string channel in new[] { "Event", "Guild", "Moderator", "Server", "Warning", "Whisper", "Zone" })
            On($"IScriptSend.Client{channel}(String, String)", _ => null);
    }

    void Services()
    {
        On("ILogService.GetLogs(LogType)", _ => new List<string>());
        On("ILogService.ClearLog(LogType)", _ => null);
        On("ILogService.DebugLog(String)", _ => null);
        On("ILogService.ScriptLog(String)", args => Aqx.Call("log", "info", args[0]));
        On("ILogService.FlashLog(String)", _ => null);
        On("IDialogService.ShowMessageBox(String, String)", args => Dismiss(args, null));
        On("IDialogService.ShowMessageBox(String, String, Boolean)", args => Dismiss(args, null));
        On("IDialogService.ShowMessageBox(String, String, String[])", args => Dismiss(args, DialogResult.Cancelled));
        On("IDialogService.ShowDialog`1(TViewModel)", _ => null);
        On("IDialogService.ShowDialog`1(TViewModel, String)", _ => null);
        On("IDialogService.ShowDialog`1(TViewModel, Action`1)", _ => null);
    }
}
