namespace Mira.Skua;

public sealed class MiraPlayer(Protocol protocol, State state) : IScriptPlayer
{
    [Unsupported] public string Guild { get => throw new NotSupportedException("IScriptPlayer.Guild"); }
    public int ID => (int)(state.Me?.Id ?? 0);
    public int XP => (int)(state.Me?.Exp ?? 0);
    public int RequiredXP => (int)(state.Me?.ExpToLevel ?? 0);
    [Unsupported] public int AccessLevel { get => throw new NotSupportedException("IScriptPlayer.AccessLevel"); set => throw new NotSupportedException("IScriptPlayer.AccessLevel"); }
    public string Cell => state.Room?.Cell ?? "";
    public string Pad => state.Room?.Pad ?? "";
    public string ServerIP => state.Session.Host;
    public int State => state.Me?.State switch { EntityState.Idle => 1, EntityState.InCombat => 2, _ => 0 };
    public bool LoggedIn => state.Session.Phase.Phase == "ready";
    public bool Playing => LoggedIn && State > 0;
    public string Username => state.Me?.Name ?? "";
    public string Password => null!;
    public bool Kicked => ((MiraBot)IScriptInterface.Instance).Kicked;
    public bool IsMember => state.Me?.Member ?? false;
    public int Health => state.Me?.Hp ?? 0;
    public int MaxHealth => state.Me?.MaxHp ?? 0;
    public int Mana => state.Me?.Mp ?? 0;
    public int MaxMana => state.Me?.MaxMp ?? 0;
    public int Level => state.Me?.Level ?? 0;
    public int Gold => (int)(state.Me?.Gold ?? 0);
    public int Coins => (int)(state.Me?.Coins ?? 0);
    public InventoryItem? CurrentClass => !Playing ? null : state.Inventory.ScriptItems.FirstOrDefault(i => i.Equipped && i.Category == ItemCategory.Class);
    public int CurrentClassRank => state.Me?.ClassRank ?? 0;
    public Monster? Target
    {
        get
        {
            var combat = (MiraCombat)IScriptInterface.Instance.Combat;
            if (combat.MonsterTarget is { } monster) return monster;
            return combat.SelectedTarget is { Kind: not "monster" } target && state.Room?.PlayersById.GetValueOrDefault(target.Id) is { Hp: > 0 } player ? new() { HP = player.Hp, MaxHP = player.MaxHp, State = (int)player.State } : new();
        }
    }
    public bool HasTarget => ((MiraCombat)IScriptInterface.Instance.Combat).HasTarget;
    public bool Loaded => state.Session.Phase.Phase == "ready";
    public SkillInfo[]? Skills => state.Me?.Skills.Select(s => s.ToScript((long)(state.Combat.SkillTiming?.GetValueOrDefault(s.Slot)?.LastUse ?? 0))).ToArray();
    public PlayerStats? Stats => state.Me?.ScriptStats;
    public bool AFK => state.Me?.Away ?? false;
    public int X => state.Room?.X ?? 0;
    public int Y => state.Room?.Y ?? 0;
    [Unsupported] public int Scale { get => throw new NotSupportedException("IScriptPlayer.Scale"); set => throw new NotSupportedException("IScriptPlayer.Scale"); }
    [Unsupported] public int WalkSpeed { get => throw new NotSupportedException("IScriptPlayer.WalkSpeed"); set => throw new NotSupportedException("IScriptPlayer.WalkSpeed"); }
    public void Goto(string name) { if (State != 1) return; try { protocol.Call(new GotoCall(name)); } catch (InvalidOperationException) { } }
    public void SetSpawnPoint(string cell, string pad) => protocol.Call(new SetSpawnPointCall(cell, pad));
    public void WalkTo(int x, int y, int speed = 8) { try { protocol.Call(new WalkCall(x, y)); } catch (InvalidOperationException) { } }
    public void Rest(bool full = false)
    {
        protocol.Call(new RestCall());
        if (full) IScriptInterface.Instance.Wait.ForTrue(() => Health >= MaxHealth && Mana >= MaxMana, 20);
    }
}
