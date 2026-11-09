namespace Aqx.Skua;

public sealed class AqxMap(Protocol protocol, State state) : IScriptMap
{
    readonly object mapItemLock = new();
    long lastMapItem = long.MinValue / 2;
    public string LastMap { get; set; } = "";
    public bool Loading => state.Session.Phase.Phase == "loading";
    string? filePath;
    RoomSnapshot? fileRoom;
    public string FilePath { get => fileRoom?.Name == state.Room?.Name && fileRoom?.File == state.Room?.File && filePath is not null ? filePath : state.Room?.File ?? ""; set { fileRoom = state.Room; filePath = value; } }
    public string FileName => FilePath.Split(['/', '\\'], StringSplitOptions.RemoveEmptyEntries).LastOrDefault() ?? "";
    public string Name => state.Room?.MapName ?? "";
    public string FullName => state.Room?.Name ?? "";
    public bool Loaded => state.Room is not null && state.Session.Phase.Phase == "ready";
    public int RoomID => state.Room?.RoomId ?? 0;
    public int PlayerCount => state.Room?.Players.Length ?? 0;
    public List<string> Cells
    {
        get
        {
            if (state.Room is null) return [];
            if (!state.Room.CellsLoaded)
            {
                try { protocol.Call(new LoadCellsCall()); }
                catch (InvalidOperationException) { return state.Room is { } room ? [room.Cell] : []; }
            }
            return state.Room?.Cells.ToList() ?? [];
        }
    }
    public List<string>? PlayerNames => state.Room?.ScriptPlayerNames;
    public List<PlayerInfo>? Players => state.Room?.ScriptPlayers;
    public List<PlayerInfo>? CellPlayers => state.Room?.ScriptCellPlayers;
    public void Reload() { }
    public void Join(string map, string cell = "Enter", string pad = "Spawn", bool ignoreCheck = false, bool autoCorrect = true)
    {
        var bot = (AqxBot)IScriptInterface.Instance;
        LastMap = map.Split('-')[0];
        if (!bot.Player.Playing || !bot.Player.Loaded || !ignoreCheck && Name == map) return;
        int attempt = 0;
        while (Name != LastMap && !bot.ShouldExit && ++attempt < bot.Options.JoinMapTries)
        {
            if (bot.Options.PrivateRooms && !map.Contains('-') || map.Contains("-1e9")) map = $"{LastMap}-{(bot.Options.PrivateNumber > 0 ? bot.Options.PrivateNumber : 100000)}";
            try { protocol.Call(new JoinCall(map, cell, pad)); }
            catch (InvalidOperationException) { }
            Jump(Name == LastMap ? cell : bot.Player.Cell, Name == LastMap ? pad : bot.Player.Pad, autoCorrect);
            bot.Sleep(bot.Options.ActionDelay);
        }
    }
    public void JoinPacket(string map, string cell = "Enter", string pad = "Spawn") => protocol.Call(new SendCall($"%xt%zm%cmd%{RoomID}%tfer%{IScriptInterface.Instance.Player.Username}%{map}%{cell}%{pad}%"));
    public void Jump(string cell, string pad, bool autoCorrect = true, bool clientOnly = false)
    {
        var bot = (AqxBot)IScriptInterface.Instance;
        bot.ThrowIfStopped();
        if (autoCorrect && state.Room is { CellsLoaded: false })
        {
            try { protocol.Call(new LoadCellsCall()); }
            catch (InvalidOperationException) { }
        }
        protocol.Call(new JumpCall(cell, pad, clientOnly, autoCorrect));
        bot.Sleep(bot.Options.ActionDelay);
        bot.Wait.ForCellChange(cell);
    }
    public void GetMapItem(int id)
    {
        var bot = (AqxBot)IScriptInterface.Instance;
        lock (mapItemLock)
        {
            int remaining = (int)Math.Max(0, 1000 - (Environment.TickCount64 - lastMapItem));
            bot.Sleep(remaining + bot.Options.ActionDelay);
            protocol.Call(new GetMapItemCall(id));
            lastMapItem = Environment.TickCount64;
        }
    }
    public PlayerInfo? GetPlayer(string username) => Players?.Find(p => p.Name.Equals(username, StringComparison.OrdinalIgnoreCase));
    [Unsupported] public List<MapItem>? FindMapItems(bool forceRefresh = false)  => throw new NotSupportedException("IScriptMap.FindMapItems");
    [Unsupported] public void ClearMapItemsCache(string? specificMap = null)  => throw new NotSupportedException("IScriptMap.ClearMapItemsCache");
}
