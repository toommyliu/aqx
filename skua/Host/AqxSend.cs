namespace Aqx.Skua;

public sealed class AqxSend(Protocol protocol, AqxFlashUtil flash) : IScriptSend
{
    public void Packet(string packet, string type = "String")
    {
        ((IFlashUtil)flash).CallGameFunction($"sfc.send{type}", packet);
    }
    public void ClientPacket(string packet, string type = "str") => protocol.Call(new InjectCall(packet));
    public void ClientEvent(string message, string sentBy = "Skua") { }
    public void ClientGuild(string message, string sentBy = "Skua") { }
    public void ClientModerator(string message, string sentBy = "Skua") { }
    public void ClientServer(string message, string sentBy = "Skua") { }
    public void ClientWarning(string message, string sentBy = "Skua") { }
    public void ClientWhisper(string message, string sentBy = "Skua") { }
    public void ClientZone(string message, string sentBy = "Skua") { }
    [Unsupported] public void Whisper(string name, string message) => throw new NotSupportedException("IScriptSend.Whisper");
    public async Task ClientPacketSpam(string packet, string type, int delay, CancellationToken token)
    {
        while (!token.IsCancellationRequested) { ClientPacket(packet, type); await Task.Delay(delay, token); }
    }
    public async Task PacketSpam(string packet, string type, int delay, CancellationToken token)
    {
        while (!token.IsCancellationRequested) { Packet(packet, type); await Task.Delay(delay, token); }
    }
}
