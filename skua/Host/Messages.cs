using System.Text.Json;
using System.Text.Json.Serialization;
using System.Collections.Concurrent;
namespace Mira.Skua;
[JsonPolymorphic(TypeDiscriminatorPropertyName = "call")]
[JsonDerivedType(typeof(JoinCall), "join")]
[JsonDerivedType(typeof(JumpCall), "jump")]
[JsonDerivedType(typeof(WalkCall), "walk")]
[JsonDerivedType(typeof(GotoCall), "goto")]
[JsonDerivedType(typeof(RestCall), "rest")]
[JsonDerivedType(typeof(RestPacketCall), "rest_packet")]
[JsonDerivedType(typeof(ExitCombatCall), "exit_combat")]
[JsonDerivedType(typeof(AttackCall), "attack")]
[JsonDerivedType(typeof(UntargetCall), "untarget")]
[JsonDerivedType(typeof(SetSkillLastUseCall), "set_skill_last_use")]
[JsonDerivedType(typeof(UseSkillCall), "use_skill")]
[JsonDerivedType(typeof(LoadQuestsCall), "load_quests")]
[JsonDerivedType(typeof(AcceptCall), "accept")]
[JsonDerivedType(typeof(CompleteCall), "complete")]
[JsonDerivedType(typeof(AbandonCall), "abandon")]
[JsonDerivedType(typeof(PickupCall), "pickup")]
[JsonDerivedType(typeof(GetMapItemCall), "get_map_item")]
[JsonDerivedType(typeof(LoadBankCall), "load_bank")]
[JsonDerivedType(typeof(LoadCellsCall), "load_cells")]
[JsonDerivedType(typeof(DepositCall), "deposit")]
[JsonDerivedType(typeof(WithdrawCall), "withdraw")]
[JsonDerivedType(typeof(SwapCall), "swap")]
[JsonDerivedType(typeof(LoadShopCall), "load_shop")]
[JsonDerivedType(typeof(BuyCall), "buy")]
[JsonDerivedType(typeof(SellCall), "sell")]
[JsonDerivedType(typeof(EquipCall), "equip")]
[JsonDerivedType(typeof(UnequipCall), "unequip")]
[JsonDerivedType(typeof(UseItemCall), "use_item")]
[JsonDerivedType(typeof(SendCall), "send")]
[JsonDerivedType(typeof(InjectCall), "inject")]
[JsonDerivedType(typeof(SetAggroCall), "set_aggro")]
[JsonDerivedType(typeof(SetSpawnPointCall), "set_spawn_point")]
[JsonDerivedType(typeof(SetCounterHoldCall), "set_counter_hold")]
[JsonDerivedType(typeof(ListenCall), "listen")]
[JsonDerivedType(typeof(LogCall), "log")]
[JsonDerivedType(typeof(StatusCall), "status")]
[JsonDerivedType(typeof(ServersCall), "servers")]
[JsonDerivedType(typeof(ReloginCall), "relogin")]
[JsonDerivedType(typeof(CancelReloginCall), "cancel_relogin")]
[JsonDerivedType(typeof(LogoutCall), "logout")]
[JsonDerivedType(typeof(SetLoginInfoCall), "set_login_info")]
public abstract record Request { public long Id { get; init; } }
public sealed record JoinCall(string Map, string Cell = "Enter", string Pad = "Spawn") : Request;
public sealed record JumpCall(string Cell, string Pad, bool ClientOnly = false) : Request;
public sealed record WalkCall(int X, int Y) : Request;
public sealed record GotoCall(string Player) : Request;
public sealed record RestCall() : Request;
public sealed record RestPacketCall() : Request;
public sealed record ExitCombatCall() : Request;
public sealed record AttackCall(long Target) : Request;
public sealed record UntargetCall() : Request;
public sealed record SetSkillLastUseCall(string Slot, long At) : Request;
public sealed record UseSkillCall(string Slot, SkillTarget? Target) : Request;
public sealed record LoadQuestsCall(long[] Ids) : Request;
public sealed record AcceptCall(long Quest) : Request;
public sealed record CompleteCall(long Quest, long? Reward, int Times, bool Special = false) : Request;
public sealed record AbandonCall(long Quest) : Request;
public sealed record PickupCall(long Item) : Request;
public sealed record GetMapItemCall(long Item) : Request;
public sealed record LoadCellsCall() : Request;
public sealed record LoadBankCall() : Request;
public sealed record DepositCall(long Item) : Request;
public sealed record WithdrawCall(long Item) : Request;
public sealed record SwapCall(long Inventory, long Bank) : Request;
public sealed record LoadShopCall(long Shop) : Request;
public sealed record BuyCall(long Listing, uint Quantity) : Request;
public sealed record SellCall(long Item, uint Quantity) : Request;
public sealed record EquipCall(long Item) : Request;
public sealed record UnequipCall(long Item) : Request;
public sealed record UseItemCall(long Item) : Request;
public sealed record SendCall(string Text) : Request;
public sealed record InjectCall(string Text) : Request;
public sealed record SetAggroCall(AggroMode Mode) : Request;
public sealed record SetSpawnPointCall(string Cell, string Pad) : Request;
public sealed record SetCounterHoldCall(CounterMode Mode) : Request;
public sealed record ListenCall(PacketDirection Direction, bool Enabled) : Request;
public sealed record LogCall(LogLevel Level, string Text) : Request;
public sealed record StatusCall(string Text) : Request;
public sealed record ServersCall() : Request;
public sealed record ReloginCall(string? Server, int TimeoutMs = 30000) : Request;
public sealed record CancelReloginCall(long Request) : Request;
public sealed record LogoutCall() : Request;
public sealed record SetLoginInfoCall(string Username, string Password) : Request;
public enum PacketDirection { In, Out }
public enum LogLevel { Log, Warn, Error }
public enum CounterMode { Off, All, Attacks }
[JsonConverter(typeof(AggroConverter))]
public sealed record AggroMode(string Mode, long[]? Only = null);
public sealed class AggroConverter : JsonConverter<AggroMode>
{
    public override AggroMode Read(ref Utf8JsonReader reader, Type type, JsonSerializerOptions options)
    {
        using var document = JsonDocument.ParseValue(ref reader);
        var value = document.RootElement;
        if (value.ValueKind == JsonValueKind.String)
        {
            string mode = value.GetString()!;
            if (mode is not ("off" or "cell" or "all")) throw new JsonException("unknown aggro mode");
            return new(mode);
        }
        return new("only", value.GetProperty("only").EnumerateArray().Select(v => v.GetInt64()).ToArray());
    }
    public override void Write(Utf8JsonWriter writer, AggroMode value, JsonSerializerOptions options)
    {
        if (value.Only is null) writer.WriteStringValue(value.Mode);
        else { writer.WriteStartObject(); writer.WritePropertyName("only"); writer.WriteStartArray(); foreach (long id in value.Only) writer.WriteNumberValue(id); writer.WriteEndArray(); writer.WriteEndObject(); }
    }
}
public enum EndOutcome { Finished, Stopped, Failed }
public sealed record Done(EndOutcome Outcome, string? Detail);
public sealed record Completion(Done Done);
public enum ErrorKind
{
    [JsonStringEnumMemberName("not_ready")] NotReady,
    Refused,
    [JsonStringEnumMemberName("no_reply")] NoReply,
    [JsonStringEnumMemberName("no_target")] NoTarget,
    Unusable, Dead,
    [JsonStringEnumMemberName("in_combat")] InCombat,
    Interrupted,
    [JsonStringEnumMemberName("still_in_combat")] StillInCombat,
    Http, Closed, Stopped, Cancelled
}
public sealed record Error(ErrorKind Kind, string Message);
public sealed class CallFailedException(ErrorKind kind, string message) : InvalidOperationException($"{kind}: {message}")
{
    public ErrorKind Kind => kind;
}
public sealed record SuccessReply(long Reply, JsonElement Ok);
public sealed record FailureReply(long Reply, Error Error);
public sealed record ReplyMessage(long Reply, JsonElement? Ok, Error? Error);
public enum SectionName { Session, Me, Room, Inventory, Temp, Drops, Bank, Quests, Factions, Shop, Combat, Settings }
public sealed record SectionUpdate(SectionName Section, JsonElement Value);
public sealed record StateMessage(SectionUpdate State);
public sealed record EventMessage(ClientEvent Event);
public sealed record Packet(PacketDirection Direction, string Text);
public sealed record PacketMessage(Packet Packet);
public sealed record Empty;
public sealed record StopMessage(Empty Stop);
public sealed record DisconnectedMessage(bool Disconnected);
public sealed record ResetMessage(Empty Reset);
[JsonSourceGenerationOptions(PropertyNamingPolicy = JsonKnownNamingPolicy.SnakeCaseLower, UseStringEnumConverter = true)]
[JsonSerializable(typeof(Request))]
[JsonSerializable(typeof(Completion))]
[JsonSerializable(typeof(SuccessReply))]
[JsonSerializable(typeof(FailureReply))]
[JsonSerializable(typeof(StateMessage))]
[JsonSerializable(typeof(EventMessage))]
[JsonSerializable(typeof(PacketMessage))]
[JsonSerializable(typeof(StopMessage))]
[JsonSerializable(typeof(DisconnectedMessage))]
[JsonSerializable(typeof(ResetMessage))]
internal partial class ProtocolJson : JsonSerializerContext;
[JsonConverter(typeof(SkillTargetConverter))]
public sealed record SkillTarget(string Kind, long Id);
public sealed class SkillTargetConverter : JsonConverter<SkillTarget>
{
    public override SkillTarget Read(ref Utf8JsonReader reader, Type type, JsonSerializerOptions options)
    {
        using var document = JsonDocument.ParseValue(ref reader);
        var field = document.RootElement.EnumerateObject().Single();
        if (field.Name is not ("monster" or "player")) throw new JsonException("unknown skill target");
        return new(field.Name, field.Value.GetInt64());
    }
    public override void Write(Utf8JsonWriter writer, SkillTarget value, JsonSerializerOptions options)
    {
        writer.WriteStartObject(); writer.WriteNumber(value.Kind, value.Id); writer.WriteEndObject();
    }
}
