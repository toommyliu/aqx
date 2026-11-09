using CommunityToolkit.Mvvm.Messaging;
using Skua.Core.Messaging;
using System.Text.Json;

namespace Aqx.Skua;

public sealed class AqxServers(Protocol protocol, State state) : IScriptServers
{
    public void SetLoginInfo(string username, string password) => protocol.Call(new SetLoginInfoCall(username, password));
    string? lastIP;
    public string LastIP { get => lastIP ?? state.Session.Host; set => lastIP = value; }
    string? lastName;
    public string LastName { get => lastName ?? state.Session.Server; set => lastName = value; }
    readonly object cacheLock = new();
    List<Server> cached = [];
    public List<Server> ServerList => GetServers().GetAwaiter().GetResult();
    public List<Server> CachedServers { get { lock (cacheLock) return cached; } }
    public event PropertyChangedEventHandler? PropertyChanged;
    public ValueTask<List<Server>> GetServers(bool forceUpdate = false)
    {
        lock (cacheLock)
        {
            if (cached.Count > 0 && !forceUpdate) return ValueTask.FromResult(cached);
            try
            {
                var servers = protocol.Call(new ServersCall())!.Value.Deserialize(ServerJson.Default.ServerSnapshotArray)!;
                var previous = cached;
                cached = servers.Select(s => new Server { Name = s.Name, IP = s.Host, Port = s.Port, Online = s.Online, Upgrade = s.MembersOnly, PlayerCount = s.Players, MaxPlayers = s.Capacity }).ToList();
                PropertyChanged?.Invoke(this, new(nameof(CachedServers)));
                WeakReferenceMessenger.Default.Send(new CommunityToolkit.Mvvm.Messaging.Messages.PropertyChangedMessage<List<Server>>(this, nameof(CachedServers), previous, cached));
                return ValueTask.FromResult(cached);
            }
            catch (InvalidOperationException) { return ValueTask.FromResult(new List<Server>()); }
        }
    }
    [Unsupported] public bool ConnectIP(string serverIp)  => throw new NotSupportedException("IScriptServers.ConnectIP");
    [Unsupported] public bool ConnectIP(string serverIp, int port)  => throw new NotSupportedException("IScriptServers.ConnectIP");
    public bool IsConnected => state.Session.Phase.Phase == "ready";
    [Unsupported] public void Login(string username, string password)  => throw new NotSupportedException("IScriptServers.Login");
    [Unsupported] public void Login()  => throw new NotSupportedException("IScriptServers.Login");
    public bool LoggingOut { get; private set; }
    public void Logout() { LoggingOut = true; LoggedOut(); protocol.Call(new LogoutCall()); }
    static void LoggedOut() => StrongReferenceMessenger.Default.Send<LogoutMessage, int>((int)MessageChannels.GameEvents);
    public bool Reconnect(string serverName, int loginDelay = 2000) { ((AqxBot)IScriptInterface.Instance).Sleep(loginDelay); return Relogin(serverName); }
    public bool Reconnect(Server server, int loginDelay = 2000) => Reconnect(server.Name, loginDelay);
    IScriptInterface Bot => IScriptInterface.Instance;
    bool Eligible(Server server) => !string.IsNullOrWhiteSpace(server.Name) && !string.IsNullOrWhiteSpace(server.IP)
        && server.Online && !server.Name.Contains("Test", StringComparison.OrdinalIgnoreCase)
        && (server.MaxPlayers <= 0 || server.PlayerCount < server.MaxPlayers) && (!server.Upgrade || Bot.Player.IsMember);
    Server? Select(string? preferred, bool exact, HashSet<string>? attempted = null)
    {
        var candidates = CachedServers.Where(Eligible).DistinctBy(s => s.Name, StringComparer.OrdinalIgnoreCase).ToList();
        Server? Find(bool onlyUntried)
        {
            var available = candidates.Where(s => !onlyUntried || attempted is null || !attempted.Contains(s.Name)).ToList();
            return available.Find(s => exact ? s.Name.Equals(preferred, StringComparison.OrdinalIgnoreCase) : preferred is not null && s.Name.Contains(preferred, StringComparison.OrdinalIgnoreCase))
                ?? (Bot.Options.AutoReloginAny ? available.Find(s => s.IP != LastIP) : null)
                ?? available.Find(s => s.Name.Equals(Bot.Options.ReloginServer, StringComparison.OrdinalIgnoreCase))
                ?? available.FirstOrDefault();
        }
        var selected = Find(true);
        if (selected is null && attempted is not null) { attempted.Clear(); selected = Find(false); }
        return selected;
    }
    bool Connect(Server server, CancellationToken token = default)
    {
        try
        {
            LoggingOut = false;
            LoggedOut();
            protocol.Call(new ReloginCall(server.Name, Bot.Options.LoginTimeout), token);
            LastName = server.Name;
            LastIP = server.IP;
            return Bot.Player.Playing;
        }
        catch (InvalidOperationException) { return false; }
        catch (OperationCanceledException) when (token.IsCancellationRequested) { return false; }
    }
    public bool Relogin(Server server = null!)
    {
        GetServers(true).GetAwaiter().GetResult();
        var target = server is not null && Eligible(server) ? server : Select(null, true);
        if (target is null) return false;
        bool automatic = Bot.Options.AutoRelogin;
        Bot.Options.AutoRelogin = false;
        try
        {
            ((AqxBot)Bot).Sleep(2000);
            Logout();
            ((AqxBot)Bot).Sleep(2000);
            return Connect(target);
        }
        finally { Bot.Options.AutoRelogin = automatic; }
    }
    public bool Relogin(string serverName)
    {
        GetServers(true).GetAwaiter().GetResult();
        return Select(serverName, false) is { } server && Connect(server);
    }
    public bool ReloginIP(string ip) => ServerList.Find(s => s.IP == ip) is { } server && Relogin(server);
    public bool EnsureRelogin(string serverName)
    {
        for (int i = 0; i < Bot.Options.ReloginTries && !Bot.ShouldExit && !Bot.Player.Playing; i++)
        {
            if (Relogin(serverName)) return true;
            if (i + 1 < Bot.Options.ReloginTries && Bot.Options.ReloginTryDelay > 0) ((AqxBot)Bot).Sleep(Bot.Options.ReloginTryDelay);
        }
        return Bot.Player.Playing;
    }
    public Task<bool> EnsureRelogin(CancellationToken token) => EnsureLogin(null, token);
    public Task<bool> EnsureRelogin(string serverName, CancellationToken token) => EnsureLogin(serverName, token);
    async Task<bool> EnsureLogin(string? serverName, CancellationToken token)
    {
        using var linked = CancellationTokenSource.CreateLinkedTokenSource(token, ((AqxBot)Bot).ScriptCts.Token);
        token = linked.Token;
        try
        {
            await GetServers(true);
            var preferred = Select(serverName, true)?.Name;
            HashSet<string> attempted = new(StringComparer.OrdinalIgnoreCase);
            for (int i = 0; i < Bot.Options.ReloginTries && !Bot.ShouldExit && !Bot.Player.Playing && !token.IsCancellationRequested; i++)
            {
                await GetServers(true);
                var server = Select(preferred, true, attempted);
                if (server is not null)
                {
                    attempted.Add(server.Name);
                    if (await Task.Run(() => Connect(server, token), token)) return true;
                }
                preferred = null;
                if (i + 1 < Bot.Options.ReloginTries && Bot.Options.ReloginTryDelay > 0) await Task.Delay(Bot.Options.ReloginTryDelay, token);
            }
        }
        catch (OperationCanceledException) { }
        return Bot.Player.Playing;
    }

}

public sealed record ServerSnapshot(string Name, string Host, int Port, bool Online, bool MembersOnly, int Players, int Capacity);

[System.Text.Json.Serialization.JsonSourceGenerationOptions(PropertyNamingPolicy = System.Text.Json.Serialization.JsonKnownNamingPolicy.SnakeCaseLower, RespectRequiredConstructorParameters = true)]
[System.Text.Json.Serialization.JsonSerializable(typeof(ServerSnapshot[]))]
internal partial class ServerJson : System.Text.Json.Serialization.JsonSerializerContext;
