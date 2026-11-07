namespace Mira.Skua;

public sealed class MiraCombat : IScriptCombat
{
    readonly Protocol protocol;
    readonly State state;
    long? savedTarget;
    sealed record Selection(SkillTarget Target, string Map, string Cell, bool Auto);
    Selection? selection;
    public SkillTarget? SelectedTarget
    {
        get
        {
            var current = Volatile.Read(ref selection);
            return current is not null && state.Room is { } room && current.Map == room.Name && current.Cell == room.Cell && (!current.Auto || state.Combat.Target == current.Target.Id) ? current.Target : null;
        }
    }
    public Monster? MonsterTarget
    {
        get
        {
            if (SelectedTarget is not { Kind: "monster" } target) return null;
            if (state.Room?.MonstersById.GetValueOrDefault(target.Id) is { Hp: > 0 } monster) return monster.ToScript();
            CancelTarget();
            return null;
        }
    }
    public bool HasTarget => SelectedTarget is { } target && (target.Kind == "monster" ? MonsterTarget is not null : target.Id == state.Me?.Id ? state.Me.Hp > 0 : state.Room?.PlayersById.GetValueOrDefault(target.Id)?.Hp > 0);
    void SelectTarget(SkillTarget target, bool auto = false) { if (state.Room is { } room) Volatile.Write(ref selection, new(target, room.Name, room.Cell, auto)); }
    public MiraCombat(Protocol protocol, State state)
    {
        this.protocol = protocol;
        this.state = state;
        protocol.EventReceived += OnEvent;
        protocol.StopRequested += () => StopAttacking = false;
    }
    void OnEvent(ClientEvent data)
    {
        if (data is DiedClientEvent) StopAttacking = false;
        if (!EnableCounterHandler) return;
        if (data is CounterStartedClientEvent)
        {
            StopAttacking = true;
            savedTarget = MonsterTarget?.MapID;
            CancelAutoAttack();
            CancelTarget();
        }
        else if (data is CounterEndedClientEvent)
        {
            StopAttacking = false;
            if (savedTarget is { } id) Attack((int)id);
            savedTarget = null;
        }
    }
    public bool EnableCounterHandler { get; set; }
    bool stopAttacking;
    public bool StopAttacking { get => Volatile.Read(ref stopAttacking); set => Volatile.Write(ref stopAttacking, value); }
    public void ApproachTarget() { if (SelectedTarget is { Kind: "monster" } target) Attack((int)target.Id); }
    public bool Attack(int id) => AttackMonster(Select(m => m.Id == id || m.MonId == id));
    public bool Attack(string name) => AttackMonster(Select(m => name == "*" || m.Name.Contains(name, StringComparison.OrdinalIgnoreCase)));
    MonsterSnapshot? Select(Func<MonsterSnapshot, bool> matches) => state.Room is { } room
        ? room.Monsters.Where(m => m.Cell == room.Cell && matches(m)).OrderByDescending(m => m.Hp > 0).ThenBy(m => m.Hp).ThenBy(m => m.Id).FirstOrDefault()
        : null;
    bool AttackMonster(MonsterSnapshot? monster)
    {
        ((MiraBot)IScriptInterface.Instance).ThrowIfStopped();
        if (StopAttacking) { CancelTarget(); return false; }
        if (monster is null) return false;
        try { if (monster.Hp > 0) protocol.Call(new AttackCall(monster.Id)); SelectTarget(new("monster", monster.Id), monster.Hp > 0); return true; }
        catch (InvalidOperationException) { return false; }
    }
    public bool AttackPlayer(string name)
    {
        long? id = state.Me?.Name.Equals(name, StringComparison.OrdinalIgnoreCase) == true ? state.Me.Id
            : state.Room?.Players.FirstOrDefault(p => p.Cell == state.Room.Cell && p.Name.Equals(name, StringComparison.OrdinalIgnoreCase))?.Eid;
        if (id is null) return false;
        protocol.Call(new UntargetCall());
        SelectTarget(new("player", id.Value));
        return true;
    }
    public void CancelAutoAttack()
    {
        if (Volatile.Read(ref selection) is { } current) Volatile.Write(ref selection, current with { Auto = false });
        protocol.Call(new UntargetCall());
    }
    public void CancelTarget() { Volatile.Write(ref selection, null); protocol.Call(new UntargetCall()); }
    public void Exit()
    {
        var bot = (MiraBot)IScriptInterface.Instance;
        if (bot.Player.State == 1 || state.Me is null) return;
        CancelAutoAttack();
        CancelTarget();
        bot.Map.Jump(bot.Player.Cell, bot.Player.Pad);
        bot.Sleep(300);
        bot.Map.Jump(bot.Player.Cell, bot.Player.Pad);
        bot.Wait.ForCombatExit();
    }
    public void UntargetSelf() { if (SelectedTarget is { Kind: "player" } target && target.Id == state.Me?.Id) CancelTarget(); }
}
