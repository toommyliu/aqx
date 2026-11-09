using System.Text.Json;
using System.Text.Json.Serialization;
namespace Aqx.Skua;

[JsonPolymorphic(TypeDiscriminatorPropertyName = "event")]
[JsonDerivedType(typeof(PhaseClientEvent), "phase")]
[JsonDerivedType(typeof(JoinedClientEvent), "joined")]
[JsonDerivedType(typeof(MovedClientEvent), "moved")]
[JsonDerivedType(typeof(PlayerEnteredClientEvent), "player_entered")]
[JsonDerivedType(typeof(PlayerLeftClientEvent), "player_left")]
[JsonDerivedType(typeof(PlayerJumpedClientEvent), "player_jumped")]
[JsonDerivedType(typeof(PlayerWalkedClientEvent), "player_walked")]
[JsonDerivedType(typeof(ReadyClientEvent), "ready")]
[JsonDerivedType(typeof(ChatClientEvent), "chat")]
[JsonDerivedType(typeof(WhisperClientEvent), "whisper")]
[JsonDerivedType(typeof(NoticeClientEvent), "notice")]
[JsonDerivedType(typeof(KilledClientEvent), "killed")]
[JsonDerivedType(typeof(DiedClientEvent), "died")]
[JsonDerivedType(typeof(RespawnedClientEvent), "respawned")]
[JsonDerivedType(typeof(LeveledUpClientEvent), "leveled_up")]
[JsonDerivedType(typeof(DropOfferedClientEvent), "drop_offered")]
[JsonDerivedType(typeof(ItemChangedClientEvent), "item_changed")]
[JsonDerivedType(typeof(BankLoadedClientEvent), "bank_loaded")]
[JsonDerivedType(typeof(ItemPickedUpClientEvent), "item_picked_up")]
[JsonDerivedType(typeof(ItemReceivedClientEvent), "item_received")]
[JsonDerivedType(typeof(ItemBoughtClientEvent), "item_bought")]
[JsonDerivedType(typeof(ItemSoldClientEvent), "item_sold")]
[JsonDerivedType(typeof(QuestAcceptedClientEvent), "quest_accepted")]
[JsonDerivedType(typeof(QuestCompletedClientEvent), "quest_completed")]
[JsonDerivedType(typeof(AuraAddedClientEvent), "aura_added")]
[JsonDerivedType(typeof(AuraRemovedClientEvent), "aura_removed")]
[JsonDerivedType(typeof(CounterStartedClientEvent), "counter_started")]
[JsonDerivedType(typeof(CounterEndedClientEvent), "counter_ended")]
[JsonDerivedType(typeof(AwayChangedClientEvent), "away_changed")]
[JsonDerivedType(typeof(ZoneClientEvent), "zone")]
[JsonDerivedType(typeof(UnreadableClientEvent), "unreadable")]
[JsonDerivedType(typeof(PacketClientEvent), "packet")]
public abstract record ClientEvent;
public sealed record PhaseClientEvent(string Phase) : ClientEvent;
public sealed record JoinedClientEvent(string Map, string Cell, string Pad, uint Players) : ClientEvent;
public sealed record MovedClientEvent(string Cell, string Pad) : ClientEvent;
public sealed record PlayerEnteredClientEvent(string Name) : ClientEvent;
public sealed record PlayerLeftClientEvent(string Name) : ClientEvent;
public sealed record PlayerJumpedClientEvent(string Name, string Cell, string Pad) : ClientEvent;
public sealed record PlayerWalkedClientEvent(string Name, string Cell, long X, long Y) : ClientEvent;
public sealed record ReadyClientEvent(uint Level, ulong Gold, uint Items) : ClientEvent;
public sealed record ChatClientEvent(string Channel, string Sender, string Text) : ClientEvent;
public sealed record WhisperClientEvent(string Sender, string Text) : ClientEvent;
public sealed record NoticeClientEvent(string Kind, string Text) : ClientEvent;
public sealed record KilledClientEvent(long Id, string Name, ulong Gold, ulong Exp) : ClientEvent;
public sealed record DiedClientEvent() : ClientEvent;
public sealed record RespawnedClientEvent() : ClientEvent;
public sealed record LeveledUpClientEvent(uint Level) : ClientEvent;
public sealed record DropOfferedClientEvent(long Id, string Name, uint Qty) : ClientEvent { [JsonIgnore] public ItemBase? Item { get; init; } }
public sealed record ItemChangedClientEvent(long Id, string Name, uint Qty, string Place) : ClientEvent;
public sealed record BankLoadedClientEvent() : ClientEvent;
public sealed record ItemPickedUpClientEvent(long Id) : ClientEvent;
public sealed record ItemReceivedClientEvent(long Id, uint Qty, string Place) : ClientEvent { [JsonIgnore] public ItemBase? Item { get; init; } }
public sealed record ItemBoughtClientEvent(long CharItemId) : ClientEvent;
public sealed record ItemSoldClientEvent(long CharItemId, uint Qty, uint QtyNow, ulong Amount, bool Coins) : ClientEvent;
public sealed record QuestAcceptedClientEvent(long Id) : ClientEvent;
public sealed record QuestCompletedClientEvent(long Id) : ClientEvent;
public sealed record AuraAddedClientEvent(JsonElement On, string Name, uint Stacks) : ClientEvent;
public sealed record AuraRemovedClientEvent(JsonElement On, string Name) : ClientEvent;
public sealed record CounterStartedClientEvent(long Monster) : ClientEvent;
public sealed record CounterEndedClientEvent(long Monster) : ClientEvent;
public sealed record AwayChangedClientEvent(bool Away) : ClientEvent;
public sealed record ZoneClientEvent(string? Zone) : ClientEvent;
public sealed record UnreadableClientEvent(string Error) : ClientEvent;
public sealed record PacketClientEvent(PacketDirection Direction, string Text) : ClientEvent;
