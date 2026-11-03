using System.Text.Json.Nodes;
using Newtonsoft.Json.Linq;
using Skua.Core.Models.Servers;

namespace Aqx.Skua;

public sealed partial class Shim
{
    List<Server>? cachedServers;
    string? lastIP;
    string? lastName;

    void Servers()
    {
        On("IScriptServers.get_CachedServers()", _ => CachedServers());
        On("IScriptServers.get_ServerList()", _ => CachedServers());
        On("IScriptServers.GetServers(Boolean)", args => new ValueTask<List<Server>>((bool)args[0]! ? FetchServers() : CachedServers()));
        On("IScriptServers.get_IsConnected()", _ => Phase is "loading" or "ready");
        On("IScriptServers.get_LastIP()", _ => lastIP ?? (string?)Aqx.Call("session")?["host"] ?? "");
        On("IScriptServers.set_LastIP(String)", args => lastIP = (string?)args[0]);
        On("IScriptServers.get_LastName()", _ => lastName ?? (string?)Aqx.Call("session")?["server"]?["name"] ?? "");
        On("IScriptServers.set_LastName(String)", args => lastName = (string?)args[0]);
        On("IScriptServers.SetLoginInfo(String, String)", _ => null);
        On("IScriptServers.Logout()", _ => Logout());
        On("IScriptServers.Relogin(String)", args => Relogin((string?)args[0]));
        On("IScriptServers.Relogin(Server)", args => Relogin(((Server?)args[0])?.Name));
        On("IScriptServers.EnsureRelogin(String)", args => EnsureRelogin((string?)args[0]));
        On("IScriptServers.EnsureRelogin(CancellationToken)", _ => Task.FromResult(Act("relogin", (object?)null)));
        On("IScriptServers.EnsureRelogin(String, CancellationToken)", args => Task.FromResult(Act("relogin", args[0])));
        On("IScriptServers.add_PropertyChanged(PropertyChangedEventHandler)", _ => null);
        On("IScriptServers.remove_PropertyChanged(PropertyChangedEventHandler)", _ => null);
    }

    object? Logout()
    {
        if (settings.GetValueOrDefault("IScriptOption.AutoRelogin") is true)
            return Act("relogin", (object?)null);
        Aqx.Call("log", "warn", "the script logged out with AutoRelogin off, so it stops here");
        Stop(true);
        return null;
    }

    bool EnsureRelogin(string? name)
    {
        int tries = SettingInt("ReloginTries");
        for (int i = 0; i < tries && !ShouldExit; i++)
        {
            if (Relogin(name))
                return true;
            if (i + 1 < tries)
                Sleep(SettingInt("ReloginTryDelay"));
        }
        return false;
    }

    bool Relogin(string? name)
    {
        bool member = (bool?)Me["member"] ?? false;
        List<Server> candidates = FetchServers().FindAll(s =>
            s.Online && !(s.Name ?? "").Contains("Test", StringComparison.OrdinalIgnoreCase)
            && !(s.MaxPlayers > 0 && s.PlayerCount >= s.MaxPlayers) && !(s.Upgrade && !member));
        string? fallback = settings.GetValueOrDefault("IScriptOption.ReloginServer") as string;
        Server? server = (string.IsNullOrWhiteSpace(name) ? null : candidates.Find(s => (s.Name ?? "").Contains(name, StringComparison.OrdinalIgnoreCase)))
            ?? candidates.Find(s => string.Equals(s.Name, fallback, StringComparison.OrdinalIgnoreCase))
            ?? candidates.FirstOrDefault();
        if (server is null)
        {
            Aqx.Call("log", "warn", "no server is open to relog onto");
            return false;
        }
        return Act("relogin", server.Name);
    }

    List<Server> CachedServers() => cachedServers ??= FetchServers();

    List<Server> FetchServers() =>
        cachedServers = Aqx.Call("servers") is JsonArray servers
            ? servers.Select(server => new JObject
            {
                ["sName"] = (string?)server!["name"],
                ["sIP"] = (string?)server["host"],
                ["iPort"] = (int?)server["port"] ?? 0,
                ["bOnline"] = (bool?)server["online"] ?? false,
                ["bUpg"] = (bool?)server["members"] ?? false,
                ["iCount"] = (int?)server["players"] ?? 0,
                ["iMax"] = (int?)server["capacity"] ?? 0,
            }.ToObject<Server>()!).ToList()
            : [];
}
