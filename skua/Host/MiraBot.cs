using Skua.Core.Skills;

namespace Mira.Skua;

public sealed class MiraBot(Protocol protocol, State state, string path) : IScriptInterface
{
    readonly object services = new();
    MiraFlashUtil? flash;
    public IFlashUtil Flash { get { lock (services) return flash ??= new(protocol, state); } }
    MiraStatus? manager;
    public IScriptStatus Manager { get { lock (services) return manager ??= new(this, path); } }
    public IScriptAuto Auto { get; } = new MiraAuto();
    IScriptBoost? boosts;
    public IScriptBoost Boosts { get { lock (services) return boosts ??= new ScriptBoost(new(() => Flash), new(() => Send), new(() => Map), new(() => Inventory), new(() => Bank), new(() => Player), new(() => Wait)); } }
    public IScriptBotStats Stats { get; } = new MiraBotStats();
    public IScriptSelfAuras Self { get; } = new MiraSelfAuras(state);
    public IScriptTargetAuras Target { get; } = new MiraTargetAuras(state);
    public IAuraMonitorService AuraMonitor { get; } = new MiraAuraMonitorService();
    IUltraBossHelper? ultraBossHelper;
    public IUltraBossHelper UltraBossHelper { get { lock (services) return ultraBossHelper ??= new global::Skua.Core.Scripts.Helpers.UltraBossHelper(CommunityToolkit.Mvvm.Messaging.StrongReferenceMessenger.Default, new(() => Player), new(() => Combat)); } }
    public IScriptCombat Combat { get; } = new MiraCombat(protocol, state);
    IScriptKill? kill;
    public IScriptKill Kill { get { lock (services) return kill ??= new ScriptKill(new(() => Wait), new(() => Options), new(() => Combat), new(() => Player), new(() => Drops), new(() => Inventory), new(() => TempInv), new(() => (IScriptManager)Manager), new(() => Map), Logger, new(() => Monsters)); } }
    IScriptHunt? hunt;
    public IScriptHunt Hunt { get { lock (services) return hunt ??= new ScriptHunt(new(() => Options), new(() => Combat), new(() => Player), new(() => Monsters), new(() => Drops), new(() => Inventory), new(() => TempInv), new(() => (IScriptManager)Manager), new(() => Map), new(() => Kill), Logger); } }
    IScriptDrop? drops;
    public IScriptDrop Drops { get { lock (services) return drops ??= new MiraDrop(protocol, state); } }
    public IScriptEvent Events { get; } = new MiraEvent(protocol, state);
    public IScriptFaction Reputation { get; } = new MiraFaction(state);
    public IScriptHouseInv House { get; } = new MiraHouseInv(protocol, state);
    public IScriptInventory Inventory { get; } = new MiraInventory(protocol, state);
    public IScriptTempInv TempInv { get; } = new MiraTempInv(state);
    public IScriptBank Bank { get; } = new MiraBank(protocol, state);
    public IScriptInventoryHelper InvHelper { get; } = new MiraInventoryHelper();
    public IScriptLite Lite { get; } = new MiraLite();
    public IScriptOption Options { get; } = new MiraOption(protocol);
    public IScriptMap Map { get; } = new MiraMap(protocol, state);
    public IScriptMonster Monsters { get; } = new MiraMonster(state);
    public IScriptPlayer Player { get; } = new MiraPlayer(protocol, state);
    public IScriptQuest Quests { get; } = new MiraQuest(protocol, state);
    IScriptSend? send;
    public IScriptSend Send { get { lock (services) return send ??= new MiraSend(protocol, (MiraFlashUtil)Flash); } }
    public IScriptShop Shops { get; } = new MiraShop(protocol, state);
    IScriptSkill? skills;
    public IScriptSkill Skills { get { lock (services) return skills ??= new ScriptSkill(new AdvancedSkillContainer(), new(() => Flash), new(() => Player), new(() => Self), new(() => Target), new(() => Combat), new(() => Options), new(() => Inventory), new(() => Wait)); } }
    IScriptWait? wait;
    public IScriptWait Wait { get { lock (services) return wait ??= new MiraWait(this, state); } }
    public IScriptServers Servers { get; } = new MiraServers(protocol, state);
    IScriptHandlers? handlers;
    public IScriptHandlers Handlers { get { lock (services) return handlers ??= new MiraHandlers(this); } }
    public ICaptureProxy GameProxy { get; } = new MiraCaptureProxy();
    public IScriptAccounts Accounts { get; } = new MiraAccounts(state);
    public IScriptOptionContainer? Config { get; set; }
    public bool ShouldExit => stop.IsCancellationRequested;
    MiraLogService? logger;
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
    public bool Disconnected { get; private set; }
    public bool Kicked { get; private set; }
    public void OnDisconnected(bool kicked) { if (((MiraServers)Servers).LoggingOut && !Options.AutoRelogin) return; Disconnected = true; Kicked = kicked; RequestStop(); }
    public bool PrepareRestart()
    {
        skills?.Stop();
        drops?.Stop();
        Quests.UnregisterAllQuests();
        Options.AutoRelogin = false;
        Options.AggroAllMonsters = false;
        Options.AggroMonsters = false;
        Options.SkipCutscenes = false;
        return handlers is not MiraHandlers timer || timer.Stop();
    }
    public void Resume() { stop.Dispose(); stop = new(); Disconnected = false; Kicked = false; stoppingHandlers = true; }
    readonly AsyncLocal<bool> cleanup = new();
    bool stoppingHandlers = true;
    public CancellationTokenSource ScriptCts => stop;
    public void RequestStop() { Combat.StopAttacking = false; stop.Cancel(); }
    public void ThrowIfStopped() { if (!cleanup.Value) stop.Token.ThrowIfCancellationRequested(); }
    public void RaiseStopping(Exception? error)
    {
        if (!stoppingHandlers) return;
        var handlers = Task.Run(() => { cleanup.Value = true; ((MiraEvent)Events).RaiseStopping(error); });
        try { handlers.Wait(TimeSpan.FromSeconds(2)); } catch (AggregateException e) { Console.Error.WriteLine(e.InnerException?.Message); }
    }
}
