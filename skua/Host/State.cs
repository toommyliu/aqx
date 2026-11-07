using System.Collections.Immutable;
using System.Text.Json;
using System.Text.Json.Serialization;
namespace Mira.Skua;

public sealed class State
{
    SessionSnapshot session = new(new("loading"), "", "");
    MeSnapshot? me;
    RoomSnapshot? room;
    InventorySnapshot inventory = new([]);
    ImmutableList<TempItemSnapshot> temp = [];
    ImmutableList<DropOfferSnapshot> drops = [];
    InventorySnapshot? bank;
    QuestsSnapshot quests = new(ImmutableDictionary<long, QuestSnapshot>.Empty, [], null, ImmutableDictionary<long, string?>.Empty);
    FactionsSnapshot factions = new([]);
    ShopSnapshot? shop;
    CombatSnapshot combat = new(null, ImmutableDictionary<string, SkillReadiness>.Empty);
    SettingsSnapshot settings = new(CounterMode.Off, new("off"), false, null);
    public SessionSnapshot Session => Volatile.Read(ref session);
    public MeSnapshot? Me => Volatile.Read(ref me);
    public RoomSnapshot? Room => Volatile.Read(ref room);
    public InventorySnapshot Inventory => Volatile.Read(ref inventory);
    public ImmutableList<TempItemSnapshot> Temp => Volatile.Read(ref temp);
    public ImmutableList<DropOfferSnapshot> Drops => Volatile.Read(ref drops);
    public InventorySnapshot? Bank => Volatile.Read(ref bank);
    public QuestsSnapshot Quests => Volatile.Read(ref quests);
    public ImmutableList<FactionSnapshot> Factions => Volatile.Read(ref factions).Items;
    public List<Faction> ScriptFactions => Volatile.Read(ref factions).ScriptItems;
    public ShopSnapshot? Shop => Volatile.Read(ref shop);
    public CombatSnapshot Combat => Volatile.Read(ref combat);
    public SettingsSnapshot Settings => Volatile.Read(ref settings);
    public void Reset()
    {
        State fresh = new();
        Volatile.Write(ref session, fresh.session);
        Volatile.Write(ref me, fresh.me);
        Volatile.Write(ref room, fresh.room);
        Volatile.Write(ref inventory, fresh.inventory);
        Volatile.Write(ref temp, fresh.temp);
        Volatile.Write(ref drops, fresh.drops);
        Volatile.Write(ref bank, fresh.bank);
        Volatile.Write(ref quests, fresh.quests);
        Volatile.Write(ref factions, fresh.factions);
        Volatile.Write(ref shop, fresh.shop);
        Volatile.Write(ref combat, fresh.combat);
        Volatile.Write(ref settings, fresh.settings);
    }
    public void Install(SectionUpdate section)
    {
        switch (section.Section)
        {
            case SectionName.Session: Volatile.Write(ref session, section.Value.Deserialize(StateJson.Default.SessionSnapshot)!); break;
            case SectionName.Me: Volatile.Write(ref me, section.Value.Deserialize(StateJson.Default.MeSnapshot)); break;
            case SectionName.Room: Volatile.Write(ref room, section.Value.Deserialize(StateJson.Default.RoomSnapshot)); break;
            case SectionName.Inventory: Volatile.Write(ref inventory, new(section.Value.Deserialize(StateJson.Default.ImmutableArrayItemSnapshot))); break;
            case SectionName.Temp: Volatile.Write(ref temp, section.Value.Deserialize(StateJson.Default.ImmutableListTempItemSnapshot)!); break;
            case SectionName.Drops: Volatile.Write(ref drops, section.Value.Deserialize(StateJson.Default.ImmutableListDropOfferSnapshot)!); break;
            case SectionName.Bank: Volatile.Write(ref bank, section.Value.ValueKind == JsonValueKind.Null ? null : new(section.Value.Deserialize(StateJson.Default.ImmutableArrayItemSnapshot))); break;
            case SectionName.Quests: Volatile.Write(ref quests, section.Value.Deserialize(StateJson.Default.QuestsSnapshot)!); break;
            case SectionName.Factions: Volatile.Write(ref factions, new(section.Value.Deserialize(StateJson.Default.ImmutableListFactionSnapshot)!)); break;
            case SectionName.Shop: Volatile.Write(ref shop, section.Value.Deserialize(StateJson.Default.ShopSnapshot)); break;
            case SectionName.Combat: Volatile.Write(ref combat, section.Value.Deserialize(StateJson.Default.CombatSnapshot)!); break;
            case SectionName.Settings: Volatile.Write(ref settings, section.Value.Deserialize(StateJson.Default.SettingsSnapshot)!); break;
            default: throw new JsonException($"unknown state section: {section.Section}");
        }
    }
}
public enum EntityState { Dead, Idle, [JsonStringEnumMemberName("in_combat")] InCombat }
public sealed record PhaseSnapshot(string Phase);
public sealed record SessionSnapshot(PhaseSnapshot Phase, string Server, string Host);
public sealed record MeSnapshot(long Id, string Name, int Level, ulong Exp, ulong ExpToLevel, ulong Gold, ulong Coins, bool Member, string ClassName, int ClassPoints, int ClassRank, long CharId, uint Aq, bool EmailVerified, int BagSlots, int HouseSlots, int BankSlots, int BankCount, ImmutableDictionary<string, ulong> Boosts, int Hp, int MaxHp, int Mp, int MaxMp, EntityState State, bool Away, StatsSnapshot Stats, ImmutableArray<SkillSnapshot> Skills, long? ItemSlot, QuestProgressSnapshot Quests, AurasSnapshot Auras)
{
    public PlayerStats ScriptStats { get; } = new() { Strength = (int)Stats.Strength, Endurance = (int)Stats.Endurance, Dexterity = (int)Stats.Dexterity, Intellect = (int)Stats.Intellect, Wisdom = (int)Stats.Wisdom, Luck = (int)Stats.Luck, AttackPower = (int)Stats.AttackPower, SpellPower = (int)Stats.SpellPower, CriticalChance = (float)Stats.Crit, CriticalMultiplier = (float)Stats.CriticalMultiplier, Haste = (float)Stats.Haste, EvasionChance = (float)Stats.Evasion };
}
public sealed record StatsSnapshot(double Strength, double Endurance, double Dexterity, double Intellect, double Wisdom, double Luck, double Hit, double Evasion, double Crit, double Haste, double AttackPower, double SpellPower, double CriticalMultiplier);
public sealed record SkillSnapshot(string Slot, string Name, int CooldownMs, int Mana, string Target, int MaxTargets, bool Unlocked, long Id = 0, long ActId = 0, double Damage = 0, string Icon = "", string Description = "", int Range = 0, string Kind = "", string Animation = "", int MinTargets = 0, bool SkillLock = false, bool Locked = false)
{
    public SkillInfo ToScript(long lastUse = 0) => new() { ID = (int)Id, ActID = (int)ActId, Damage = Damage, Icon = Icon, Description = Description, Range = Range, Type = Kind, _Anim = Animation, MinTargets = MinTargets, Name = Name, Auto = Slot == "aa", Cooldown = CooldownMs, MP = Mana, MaxTargets = MaxTargets, IsOk = Unlocked, Locked = Locked, LastUse = lastUse };
}
public sealed record RoomSnapshot(string Name, string Cell, string Pad, int X, int Y, ImmutableArray<PlayerSnapshot> Players, ImmutableArray<MonsterSnapshot> Monsters, string? Zone, int RoomId = 0, ImmutableArray<string> Cells = default, string File = "", bool CellsLoaded = false)
{
    public string MapName { get; } = Name.Split('-')[0];
    public List<PlayerInfo> ScriptPlayers { get; } = Players.Select(ToPlayer).ToList();
    public List<PlayerInfo> ScriptCellPlayers { get; } = Players.Where(p => p.Cell == Cell).Select(ToPlayer).ToList();
    public List<string> ScriptPlayerNames { get; } = Players.Select(p => p.Name).ToList();
    static PlayerInfo ToPlayer(PlayerSnapshot p) => new() { Name = p.Name, EntID = (int)p.Eid, Cell = p.Cell!, Pad = p.Pad!, HP = p.Hp, MaxHP = p.MaxHp, MP = p.Mp, AFK = p.Away, Level = p.Level ?? 0, X = p.X, Y = p.Y };
    public ImmutableDictionary<long, PlayerSnapshot> PlayersById { get; } = Players.ToImmutableDictionary(p => p.Eid);
    public ImmutableDictionary<long, MonsterSnapshot> MonstersById { get; } = Monsters.ToImmutableDictionary(p => p.Id);
}
public sealed record PlayerSnapshot(long Eid, string Name, string? Cell, string? Pad, int X, int Y, int? Level, string? ClassName, int Hp, int MaxHp, int Mp, int MaxMp, EntityState State, bool Away, AurasSnapshot? Auras = null);
public sealed record MonsterSnapshot(long Id, long MonId, string Name, string Cell, int Hp, int MaxHp, EntityState State, bool Aggressive, AurasSnapshot Auras, string Race = "", string File = "")
{
    public Monster ToScript() => new() { ID = (int)MonId, MapID = (int)Id, Name = Name, Race = Race, FileName = File, Cell = Cell, HP = Hp, MaxHP = MaxHp, State = (int)State };
}
public enum InventoryPool { Bag, Misc, Class, House }
public sealed record ItemSnapshot(long Id, long CharItemId, string Name, int Qty, int MaxStack, string Kind, string Slot, bool Equipped, bool Coins, bool Member, string Meta, string Link, string File, bool House, EnhancementSnapshot? Enhancement, ItemDetailsSnapshot Details)
{
    public InventoryPool Pool { get; } = Classify(Kind, Meta, House);
    public static InventoryPool Classify(string kind, string meta, bool house) => kind switch
    {
        "Class" => InventoryPool.Class,
        "House" or "Wall Item" or "Floor Item" or "Guild" => InventoryPool.House,
        _ when house => InventoryPool.House,
        "Item" when meta.Trim() is { Length: > 0 } digits && digits.All(c => c is >= '0' and <= '9') => InventoryPool.Bag,
        "Item" or "Note" or "Quest Item" or "Resource" => InventoryPool.Misc,
        _ => InventoryPool.Bag
    };
    public InventoryItem ToScript() => new InventoryEntry { Description = Details.Description, Level = Details.Level, Cost = Details.Cost, Hours = Details.Hours, QuestSlot = Details.QuestSlot, QuestValue = Details.QuestValue, Quest = Details.Quest, ID = (int)Id, CharItemID = (int)CharItemId, Name = Name, Quantity = Qty, MaxStack = MaxStack, CategoryString = Kind, ItemGroup = Slot, Equipped = Equipped, Coins = Coins, Upgrade = Member, Meta = Meta, FileName = Link, FileLink = File, House = House, EnhancementLevel = Enhancement?.Level ?? 0, EnhancementPatternID = Enhancement?.Pattern ?? 0, ProcID = Enhancement?.ProcId ?? 0 };
}
public sealed record ItemDetailsSnapshot(string Description, int Level, ulong Cost, int? Hours, long? QuestSlot, long QuestValue, string Quest);
public sealed class InventoryEntry : InventoryItem
{
    [Newtonsoft.Json.JsonProperty("iCost")] public ulong Cost { get; init; }
    [Newtonsoft.Json.JsonProperty("iHrs")] public int? Hours { get; init; }
    [Newtonsoft.Json.JsonProperty("iQSindex")] public long? QuestSlot { get; init; }
    [Newtonsoft.Json.JsonProperty("iQSvalue")] public long QuestValue { get; init; }
    [Newtonsoft.Json.JsonProperty("sQuest")] public string Quest { get; init; } = "";
}
public sealed record TempItemSnapshot(long Id, string Name, int Qty, int MaxStack, ItemDetailsSnapshot Details, string Kind, string Slot, string Meta, bool Coins, bool Member, string Link, string File, bool House)
{
    public ItemBase ToScript() => new() { ID = (int)Id, Name = Name, Quantity = Qty, MaxStack = MaxStack, Temp = true, Description = Details.Description, CategoryString = Kind, ItemGroup = Slot, Meta = Meta, Coins = Coins, Upgrade = Member, FileName = Link, FileLink = File, House = House };
}
public sealed record DropOfferSnapshot(long Id, string Name, int Qty, int MaxStack, string Kind, string Slot, bool Coins, bool Member, string Meta, string Link, string File, bool House, ItemDetailsSnapshot Details)
{
    public ItemBase ToScript() => new() { ID = (int)Id, Name = Name, Quantity = Qty, MaxStack = MaxStack, CategoryString = Kind, ItemGroup = Slot, Coins = Coins, Upgrade = Member, Meta = Meta, FileName = Link, FileLink = File, House = House, Description = Details.Description };
}
public sealed record ShopSnapshot(long Id, string Name, bool Member, bool Limited, ImmutableArray<string> Maps, bool House, ImmutableArray<ShopItemSnapshot> Items, long Index, string Field, long RequiredItem)
{
    public List<ShopItem> ScriptItems { get; } = Items.Select(i => i.ToScript()).ToList();
}
public sealed record ShopItemSnapshot(long Listing, long Id, string Name, ulong Cost, bool Coins, bool Member, int MaxStack, int Qty, long? Faction, int RequiredRep, string Kind, string Slot, string Meta, string Link, string File, bool House, ImmutableArray<MergeCostSnapshot> Merge, long EnhancementId, long Token, int TokenQty, long QuestSlot, long QuestValue, string Quest, int Level, string FactionName, string TokenName, int? Remaining)
{
    public ShopItem ToScript() => new ShopListing { ID = (int)Id, ShopItemID = (int)Listing, Name = Name, Cost = (int)Cost, Coins = Coins, Upgrade = Member, MaxStack = MaxStack, Quantity = Qty, RequiredReputation = RequiredRep, CategoryString = Kind, ItemGroup = Slot, Meta = Meta, FileName = Link, FileLink = File, House = House, TokenID = (int)Token, TokenQuantity = TokenQty, TokenName = TokenName, Level = Level, Faction = FactionName, FactionID = Faction ?? 0, EnhancementID = EnhancementId, QuestSlot = QuestSlot, QuestValue = QuestValue, Quest = Quest, Remaining = Remaining, Requirements = Merge.Select(r => new ItemBase { ID = (int)r.Id, Name = r.Name, Quantity = r.Qty }).ToList() };
}
public sealed class ShopListing : ShopItem
{
    [Newtonsoft.Json.JsonProperty("FactionID")] public long FactionID { get; init; }
    [Newtonsoft.Json.JsonProperty("EnhID")] public long EnhancementID { get; init; }
    [Newtonsoft.Json.JsonProperty("iQSindex")] public long QuestSlot { get; init; }
    [Newtonsoft.Json.JsonProperty("iQSvalue")] public long QuestValue { get; init; }
    [Newtonsoft.Json.JsonProperty("sQuest")] public string Quest { get; init; } = "";
    [Newtonsoft.Json.JsonProperty("iQtyRemain")] public int? Remaining { get; init; }
}
public sealed record MergeCostSnapshot(long Id, string Name, int Qty);
public sealed record QuestSnapshot(long Id, string Name, ImmutableArray<QuestItemSnapshot> TurnIn, ImmutableArray<QuestItemSnapshot> Required, ImmutableArray<QuestRewardSnapshot> Rewards, bool Once, long Slot, long Value, QuestPeriodSnapshot? Period, bool Member, long Level, [property: JsonPropertyName("class")] long? ClassItem, long RequiredCp, long? Faction, long RequiredRep, long Gold, long Exp, string Description = "", string EndText = "");
public sealed record QuestItemSnapshot(long Id, string Name, int Qty, int MaxStack, bool Temp);
public sealed record QuestRewardSnapshot(long Id, string Name, int Qty, int MaxStack, bool Temp, double Chance, string Kind);
public sealed record QuestPeriodSnapshot(string Field, long Index);
public sealed record FactionsSnapshot(ImmutableList<FactionSnapshot> Items)
{
    public List<Faction> ScriptItems { get; } = Items.Select(f => new Faction { ID = (int)f.Id, Name = f.Name, Rank = f.Rank, TotalRep = f.Rep, Rep = f.RankProgress, RequiredRep = f.RankProgressMax ?? 0 }).ToList();
}
public sealed record FactionSnapshot(long Id, string Name, int Rep, int Rank, int RankProgress, int? RankProgressMax);
public sealed record AuraState(string Name, double Duration, int Stacks, double? Value, string? Category, bool Persist, AuraCaster? Caster, ImmutableArray<StatEffectSnapshot> Effects, ulong? Expires, ulong Applied, string Icon = "", string Description = "");
public sealed record HudAuraState(string Name, string Icon, string Description, int Stacks, bool Persist, double Duration, ulong? Expires);
public sealed record AurasSnapshot(ImmutableArray<AuraState> Active, ImmutableArray<HudAuraState> Hud);
public sealed record StatEffectSnapshot(string Stat, string Op, double Value);
public sealed record EnhancementSnapshot(long Id, int Level, int Pattern, int ProcId, int Plus);
public sealed record QuestProgressSnapshot(ImmutableArray<string?> Strings, ImmutableDictionary<string, long> Achievements);
public sealed record InventorySnapshot(ImmutableArray<ItemSnapshot> Items)
{
    public List<InventoryItem> ScriptItems { get; } = Items.Select(i => i.ToScript()).ToList();
    public List<InventoryItem> InventoryItems { get; } = Items.Where(i => i.Pool != InventoryPool.House).Select(i => i.ToScript()).ToList();
    public List<InventoryItem> HouseItems { get; } = Items.Where(i => i.Pool == InventoryPool.House).Select(i => i.ToScript()).ToList();
    public int BagUsed { get; } = Items.Count(i => i.Pool == InventoryPool.Bag);
    public int MiscUsed { get; } = Items.Count(i => i.Pool == InventoryPool.Misc);
    public ImmutableDictionary<long, ItemSnapshot> ById { get; } = Items.ToImmutableDictionary(i => i.Id);
    public ImmutableDictionary<string, ItemSnapshot> ByName { get; } = Items.GroupBy(i => i.Name, StringComparer.OrdinalIgnoreCase).ToImmutableDictionary(g => g.Key, g => g.First(), StringComparer.OrdinalIgnoreCase);
}
public sealed record QuestsSnapshot(ImmutableDictionary<long, QuestSnapshot> Definitions, ImmutableArray<long> Accepted, QuestProgressSnapshot? Progress, ImmutableDictionary<long, string?> Status)
{
    public List<Quest> ScriptTree { get; } = Definitions.Values.Select(q => QuestModels.Project(q, Status.GetValueOrDefault(q.Id))).ToList();
}
public sealed record CombatSnapshot(long? Target, ImmutableDictionary<string, SkillReadiness> Readiness, ulong? GlobalCooldown = null, ImmutableDictionary<string, SkillTiming>? SkillTiming = null, ImmutableDictionary<string, ActionLock>? ActionLocks = null);
public sealed record SkillTiming(ulong? LastUse, ulong? Cooldown);
public sealed record ActionLock(ulong? Used, ulong Cooldown);
public sealed record SettingsSnapshot(CounterMode CounterHold, AggroMode Aggro, bool AntiAfk, uint? Room);
[JsonConverter(typeof(ReadinessConverter))]
public sealed record SkillReadiness(string Kind, ulong? At = null);
public sealed class ReadinessConverter : JsonConverter<SkillReadiness>
{
    public override SkillReadiness Read(ref Utf8JsonReader reader, Type type, JsonSerializerOptions options)
    {
        using var document = JsonDocument.ParseValue(ref reader);
        var value = document.RootElement;
        return value.ValueKind == JsonValueKind.String ? new(value.GetString()!) : new("at", value.GetProperty("at").GetUInt64());
    }
    public override void Write(Utf8JsonWriter writer, SkillReadiness value, JsonSerializerOptions options)
    {
        if (value.At is not { } at) writer.WriteStringValue(value.Kind);
        else { writer.WriteStartObject(); writer.WriteNumber("at", at); writer.WriteEndObject(); }
    }
}
[JsonConverter(typeof(CasterConverter))]
public sealed record AuraCaster(string Kind, long Id);
public sealed class CasterConverter : JsonConverter<AuraCaster>
{
    public override AuraCaster Read(ref Utf8JsonReader reader, Type type, JsonSerializerOptions options)
    {
        using var document = JsonDocument.ParseValue(ref reader);
        var field = document.RootElement.EnumerateObject().Single();
        if (field.Name is not ("player" or "monster")) throw new JsonException("unknown aura caster");
        return new(field.Name, field.Value.GetInt64());
    }
    public override void Write(Utf8JsonWriter writer, AuraCaster value, JsonSerializerOptions options)
    {
        writer.WriteStartObject(); writer.WriteNumber(value.Kind, value.Id); writer.WriteEndObject();
    }
}
[JsonSourceGenerationOptions(PropertyNamingPolicy = JsonKnownNamingPolicy.SnakeCaseLower, RespectRequiredConstructorParameters = true, UseStringEnumConverter = true)]
[JsonSerializable(typeof(SessionSnapshot))]
[JsonSerializable(typeof(MeSnapshot))]
[JsonSerializable(typeof(RoomSnapshot))]
[JsonSerializable(typeof(ImmutableArray<ItemSnapshot>))]
[JsonSerializable(typeof(ImmutableList<TempItemSnapshot>))]
[JsonSerializable(typeof(ImmutableList<DropOfferSnapshot>))]
[JsonSerializable(typeof(QuestsSnapshot))]
[JsonSerializable(typeof(ImmutableList<FactionSnapshot>))]
[JsonSerializable(typeof(ShopSnapshot))]
[JsonSerializable(typeof(CombatSnapshot))]
[JsonSerializable(typeof(SettingsSnapshot))]
internal partial class StateJson : JsonSerializerContext;
