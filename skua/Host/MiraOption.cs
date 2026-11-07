using CommunityToolkit.Mvvm.Messaging;
using CommunityToolkit.Mvvm.Messaging.Messages;


namespace Mira.Skua;

public sealed class MiraOption : IScriptOption
{
    readonly Protocol protocol;
    readonly Dictionary<string, string>? saved;
    static readonly System.Reflection.PropertyInfo[] properties = typeof(MiraOption).GetProperties().Where(p => p.CanWrite).ToArray();
    public MiraOption(Protocol protocol, State state) : this(protocol, true)
    {
        protocol.StateReceived += section => { if (section.Section == SectionName.Settings && (aggroAllMonsters || aggroMonsters) && state.Settings.Aggro.Mode == "off") SendAggro(); };
    }
    MiraOption(Protocol protocol, bool load)
    {
        this.protocol = protocol;
        if (load) { saved = MiraSettings.Read<System.Collections.Specialized.StringCollection>("UserOptions")?.Cast<string>().Select(s => s.Split('=', 2)).ToDictionary(parts => parts[0], parts => parts[1]); Reset(); }
    }
    public ImmutableDictionary<string, Func<object>> OptionDictionary => properties.ToImmutableDictionary(p => p.Name, p => (Func<object>)(() => p.GetValue(this)!));
    int reloginTryDelay = 800;
    public int ReloginTryDelay { get => reloginTryDelay; set => Set(ref reloginTryDelay, value); }
    int loginTimeout = 30000;
    public int LoginTimeout { get => loginTimeout; set => Set(ref loginTimeout, value); }
    bool acceptACDrops;
    public bool AcceptACDrops { get => acceptACDrops; set { bool previous = acceptACDrops; Set(ref acceptACDrops, value); if (previous != value) DropOptionChanged(nameof(AcceptACDrops), previous, value); } }
    bool acceptAllDrops;
    public bool AcceptAllDrops { get => acceptAllDrops; set { bool previous = acceptAllDrops; Set(ref acceptAllDrops, value); if (previous == value) return; if (value) RejectAllDrops = false; DropOptionChanged(nameof(AcceptAllDrops), previous, value); } }
    bool rejectAllDrops;
    public bool RejectAllDrops { get => rejectAllDrops; set { bool previous = rejectAllDrops; Set(ref rejectAllDrops, value); if (previous == value) return; if (value) AcceptAllDrops = false; DropOptionChanged(nameof(RejectAllDrops), previous, value); } }
    volatile bool aggroAllMonsters;
    public bool AggroAllMonsters
    {
        get => aggroAllMonsters;
        set { aggroAllMonsters = value; SendAggro(); }
    }
    bool aggroMonsters;
    public bool AggroMonsters
    {
        get => aggroMonsters;
        set { if (aggroMonsters == value) return; Set(ref aggroMonsters, value); SendAggro(); }
    }
    void SendAggro() => protocol.Call(new SetAggroCall(new(aggroAllMonsters ? "all" : aggroMonsters ? "cell" : "off")));
    bool attackWithoutTarget;
    public bool AttackWithoutTarget { get => attackWithoutTarget; set => Set(ref attackWithoutTarget, value); }
    bool autoRelogin;
    public bool AutoRelogin { get => autoRelogin; set => Set(ref autoRelogin, value); }
    bool autoReloginAny;
    public bool AutoReloginAny { get => autoReloginAny; set => Set(ref autoReloginAny, value); }
    bool retryRelogin = true;
    public bool RetryRelogin { get => retryRelogin; set => Set(ref retryRelogin, value); }
    string customGuild = string.Empty;
    public string CustomGuild { get => customGuild; set => Set(ref customGuild, value); }
    string customName = string.Empty;
    public string CustomName { get => customName; set => Set(ref customName, value); }
    bool disableCollisions;
    public bool DisableCollisions { get => disableCollisions; set => Set(ref disableCollisions, value); }
    bool disableDeathAds;
    public bool DisableDeathAds { get => disableDeathAds; set => Set(ref disableDeathAds, value); }
    bool disableFX;
    public bool DisableFX { get => disableFX; set => Set(ref disableFX, value); }
    int maximumTries = 10;
    public int MaximumTries { get => maximumTries; set => Set(ref maximumTries, value); }
    int actionDelay = 800;
    public int ActionDelay { get => actionDelay; set => Set(ref actionDelay, value); }
    int guildColor;
    public int GuildColor { get => guildColor; set => Set(ref guildColor, value); }
    bool hidePlayers;
    public bool HidePlayers { get => hidePlayers; set => Set(ref hidePlayers, value); }
    int huntBuffer = 1;
    public int HuntBuffer { get => huntBuffer; set => Set(ref huntBuffer, value); }
    int huntDelay = 1000;
    public int HuntDelay { get => huntDelay; set => Set(ref huntDelay, value); }
    bool infiniteRange;
    public bool InfiniteRange { get => infiniteRange; set => Set(ref infiniteRange, value); }
    bool lagKiller;
    public bool LagKiller { get => lagKiller; set => Set(ref lagKiller, value); }
    int loadTimeout = 30000;
    public int LoadTimeout { get => loadTimeout; set => Set(ref loadTimeout, value); }
    string? reloginServer = "Twilly";
    public string? ReloginServer { get => reloginServer; set => Set(ref reloginServer, value); }
    bool magnetise;
    public bool Magnetise { get => magnetise; set => Set(ref magnetise, value); }
    int nameColor = 0xFFFFFF;
    public int NameColor { get => nameColor; set => Set(ref nameColor, value); }
    bool privateRooms;
    public bool PrivateRooms { get => privateRooms; set => Set(ref privateRooms, value); }
    volatile bool restPackets;
    public bool RestPackets { get => restPackets; set => restPackets = value; }
    bool safeRelogin;
    public bool SafeRelogin { get => safeRelogin; set => Set(ref safeRelogin, value); }
    bool safeTimings = true;
    public bool SafeTimings { get => safeTimings; set => Set(ref safeTimings, value); }
    int setFPS = 30;
    public int SetFPS { get => setFPS; set => Set(ref setFPS, value); }
    bool showFPS = false;
    public bool ShowFPS { get => showFPS; set => Set(ref showFPS, value); }
    bool skipCutscenes;
    public bool SkipCutscenes { get => skipCutscenes; set => Set(ref skipCutscenes, value); }
    int walkSpeed = 8;
    public int WalkSpeed { get => walkSpeed; set => Set(ref walkSpeed, value); }
    int privateNumber = 0;
    public int PrivateNumber { get => privateNumber; set => Set(ref privateNumber, value); }
    int joinMapTries = 3;
    public int JoinMapTries { get => joinMapTries; set => Set(ref joinMapTries, value); }
    int questAcceptAndCompleteTries = 30;
    public int QuestAcceptAndCompleteTries { get => questAcceptAndCompleteTries; set => Set(ref questAcceptAndCompleteTries, value); }
    int reloginTries = 5;
    public int ReloginTries { get => reloginTries; set => Set(ref reloginTries, value); }
    HuntPriorities huntPriority = HuntPriorities.None;
    public HuntPriorities HuntPriority { get => huntPriority; set => Set(ref huntPriority, value); }
    public void Reset() { if (saved is null) { ResetToDefault(); return; } foreach (var p in properties) if (saved.TryGetValue(p.Name, out var value)) p.SetValue(this, p.PropertyType.IsEnum ? Enum.Parse(p.PropertyType, value, true) : Convert.ChangeType(value, p.PropertyType)); }
    public void ResetToDefault() { var defaults = new MiraOption(protocol, false); foreach (var p in properties) p.SetValue(this, p.GetValue(defaults)); }
    public void Save() { System.Collections.Specialized.StringCollection options = []; options.AddRange(properties.Select(p => $"{p.Name}={p.GetValue(this)}").ToArray()); MiraSettings.Write("UserOptions", options); }
    public event PropertyChangedEventHandler? PropertyChanged;
    void DropOptionChanged(string name, bool oldValue, bool newValue) => WeakReferenceMessenger.Default.Send(new PropertyChangedMessage<bool>(this, name, oldValue, newValue));
    void Set<T>(ref T field, T value, [System.Runtime.CompilerServices.CallerMemberName] string? name = null)
    {
        if (EqualityComparer<T>.Default.Equals(field, value)) return;
        field = value;
        PropertyChanged?.Invoke(this, new(name));
    }
}
