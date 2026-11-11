using Skua.Core.Skills;

namespace Aqx.Skua;

public sealed class AqxBot(Protocol protocol, State state, string path) : IScriptInterface
{
    readonly object services = new();
    AqxFlashUtil? flash;
    public IFlashUtil Flash { get { lock (services) return flash ??= new(protocol, state, (AqxEvent)Events); } }
    AqxStatus? manager;
    public IScriptStatus Manager { get { lock (services) return manager ??= new(this, path); } }
    public IScriptAuto Auto { get; } = new AqxAuto();
    IScriptBoost? boosts;
    public IScriptBoost Boosts { get { lock (services) return boosts ??= new ScriptBoost(new(() => Flash), new(() => Send), new(() => Map), new(() => Inventory), new(() => Bank), new(() => Player), new(() => Wait)); } }
    public IScriptBotStats Stats { get; } = new AqxBotStats();
    public IScriptSelfAuras Self { get; } = new AqxSelfAuras(state);
    public IScriptTargetAuras Target { get; } = new AqxTargetAuras(state);
    public IAuraMonitorService AuraMonitor { get; } = new AqxAuraMonitorService();
    IUltraBossHelper? ultraBossHelper;
    public IUltraBossHelper UltraBossHelper { get { lock (services) return ultraBossHelper ??= new global::Skua.Core.Scripts.Helpers.UltraBossHelper(CommunityToolkit.Mvvm.Messaging.StrongReferenceMessenger.Default, new(() => Player), new(() => Combat)); } }
    public IScriptCombat Combat { get; } = new AqxCombat(protocol, state);
    IScriptKill? kill;
    public IScriptKill Kill { get { lock (services) return kill ??= new ScriptKill(new(() => Wait), new(() => Options), new(() => Combat), new(() => Player), new(() => Drops), new(() => Inventory), new(() => TempInv), new(() => (IScriptManager)Manager), new(() => Map), Logger, new(() => Monsters)); } }
    IScriptHunt? hunt;
    public IScriptHunt Hunt { get { lock (services) return hunt ??= new ScriptHunt(new(() => Options), new(() => Combat), new(() => Player), new(() => Monsters), new(() => Drops), new(() => Inventory), new(() => TempInv), new(() => (IScriptManager)Manager), new(() => Map), new(() => Kill), Logger); } }
    IScriptDrop? drops;
    public IScriptDrop Drops { get { lock (services) return drops ??= new AqxDrop(protocol, state); } }
    public IScriptEvent Events { get; } = new AqxEvent(protocol, state);
    public IScriptFaction Reputation { get; } = new AqxFaction(state);
    public IScriptHouseInv House { get; } = new AqxHouseInv(protocol, state);
    public IScriptInventory Inventory { get; } = new AqxInventory(protocol, state);
    public IScriptTempInv TempInv { get; } = new AqxTempInv(state);
    public IScriptBank Bank { get; } = new AqxBank(protocol, state);
    public IScriptInventoryHelper InvHelper { get; } = new AqxInventoryHelper();
    public IScriptLite Lite { get; } = new AqxLite();
    public IScriptOption Options { get; } = new AqxOption(protocol, state);
    public IScriptMap Map { get; } = new AqxMap(protocol, state);
    public IScriptMonster Monsters { get; } = new AqxMonster(state);
    public IScriptPlayer Player { get; } = new AqxPlayer(protocol, state);
    public IScriptQuest Quests { get; } = new AqxQuest(protocol, state);
    IScriptSend? send;
    public IScriptSend Send { get { lock (services) return send ??= new AqxSend(protocol, (AqxFlashUtil)Flash); } }
    public IScriptShop Shops { get; } = new AqxShop(protocol, state);
    IScriptSkill? skills;
    public IScriptSkill Skills { get { lock (services) return skills ??= new ScriptSkill(new AdvancedSkillContainer(), new(() => Flash), new(() => Player), new(() => Self), new(() => Target), new(() => Combat), new(() => Options), new(() => Inventory), new(() => Wait)); } }
    IScriptWait? wait;
    public IScriptWait Wait { get { lock (services) return wait ??= new AqxWait(this, state); } }
    public IScriptServers Servers { get; } = new AqxServers(protocol, state);
    IScriptHandlers? handlers;
    public IScriptHandlers Handlers { get { lock (services) return handlers ??= new AqxHandlers(this); } }
    public ICaptureProxy GameProxy { get; } = new AqxCaptureProxy();
    public IScriptAccounts Accounts { get; } = new AqxAccounts(state);
    public IScriptOptionContainer? Config { get; set; }
    public bool ShouldExit => stop.IsCancellationRequested;
    AqxLogService? logger;
    public ILogService Logger { get { lock (services) return logger ??= new(protocol); } }
    public Version Version => null!;
    public Random Random { get; set; } = new();
    [Unsupported] public void SellJunk()  => throw new NotSupportedException("IScriptInterface.SellJunk");
    public void Sleep(int ms) { if (cleanup.Value) Thread.Sleep(ms); else { ThrowIfStopped(); if (stop.Token.WaitHandle.WaitOne(ms)) ThrowIfStopped(); } }
    public Task SleepAsync(int ms) => Task.Delay(ms, cleanup.Value ? CancellationToken.None : stop.Token);
    public void Stop(bool runScriptStoppingEvent = true) { stoppingHandlers = runScriptStoppingEvent; RequestStop(); ThrowIfStopped(); }
    public Task StopAsync(bool runScriptStoppingEvent = true) { Stop(runScriptStoppingEvent); return Task.CompletedTask; }
    public void Log(string message) => Logger.ScriptLog(message);
    [Unsupported] public Task Schedule(int delay, Action<IScriptInterface> action)  => throw new NotSupportedException("IScriptInterface.Schedule");
    [Unsupported] public Task Schedule(int delay, Func<IScriptInterface, Task> function)  => throw new NotSupportedException("IScriptInterface.Schedule");
    public bool? ShowMessageBox(string message, string caption, bool yesAndNo = false) { Log($"{caption}: {message} (dismissed, as if the window were closed)"); return null; }
    public DialogResult ShowMessageBox(string message, string caption, params string[] buttons) { ShowMessageBox(message, caption, false); return DialogResult.Cancelled; }

    CancellationTokenSource stop = new();
    public bool AccountAutoRelogin { get; set; }
    public bool Disconnected { get; private set; }
    public bool Kicked { get; private set; }
    public void OnDisconnected(bool kicked) { if (((AqxServers)Servers).LoggingOut && !Options.AutoRelogin) return; Disconnected = true; Kicked = kicked; RequestStop(); }
    public bool PrepareRestart()
    {
        skills?.Stop();
        drops?.Stop();
        Quests.UnregisterAllQuests();
        Options.AutoRelogin = AccountAutoRelogin;
        Options.AggroAllMonsters = false;
        Options.AggroMonsters = false;
        Options.SkipCutscenes = false;
        return handlers is not AqxHandlers timer || timer.Stop();
    }
    public void Resume() { stop.Dispose(); stop = new(); Disconnected = false; Kicked = false; stoppingHandlers = true; }
    readonly AsyncLocal<bool> cleanup = new();
    bool stoppingHandlers = true;
    public CancellationTokenSource ScriptCts => stop;
    public void RequestStop() { Combat.StopAttacking = false; stop.Cancel(); }
    public void ThrowIfStopped() { if (!cleanup.Value) stop.Token.ThrowIfCancellationRequested(); }
    public void ReportRegistered() => protocol.ReportRegistered(Drops.ToPickup.ToArray(), Drops.ToPickupIDs.ToArray(), ((AqxQuest)Quests).RegisteredRewards);
    public void Edit(System.Text.Json.JsonElement change)
    {
        var field = change.EnumerateObject().Single();
        var value = field.Value;
        bool byId = value.ValueKind == System.Text.Json.JsonValueKind.Number;
        switch (field.Name)
        {
            case "register_drop": if (byId) Drops.Add(value.GetInt32()); else Drops.Add(value.GetString()!); break;
            case "unregister_drop": if (byId) Drops.Remove(value.GetInt32()); else Drops.Remove(value.GetString()!); break;
            case "register_quest": Quests.RegisterQuests((value.GetProperty("id").GetInt32(), RewardId(value))); break;
            case "unregister_quest": Quests.UnregisterQuests(value.GetInt32()); break;
            default: throw new System.Text.Json.JsonException($"unknown edit {field.Name}");
        }
    }
    static int RewardId(System.Text.Json.JsonElement quest)
    {
        if (!quest.TryGetProperty("reward", out var reward)) return -1;
        if (reward.ValueKind != System.Text.Json.JsonValueKind.Number) throw new InvalidOperationException("Skua scripts take a quest reward by its item ID");
        return reward.GetInt32();
    }
    public Task RaiseStopping(Exception? error) => stoppingHandlers ? Task.Run(() => { cleanup.Value = true; ((AqxEvent)Events).RaiseStopping(error); }) : Task.CompletedTask;
}
