using System.Text.Json.Nodes;
using Skua.Core.Models.Factions;
using Skua.Core.Models.Items;

namespace Aqx.Skua;

public sealed partial class Shim
{
    const int RestPolls = 20;
    const int RestPoll = 100;

    void Player()
    {
        On("IScriptPlayer.get_LoggedIn()", _ => Phase is "loading" or "ready");
        On("IScriptPlayer.get_Loaded()", _ => Phase == "ready");
        On("IScriptPlayer.get_Playing()", _ => Playing);
        On("IScriptPlayer.get_State()", _ => State(Me));
        On("IScriptPlayer.get_Username()", _ => (string?)Me["name"] ?? "");
        On("IScriptPlayer.get_Cell()", _ => (string?)Me["cell"] ?? "");
        On("IScriptPlayer.get_Pad()", _ => (string?)Me["pad"] ?? "");
        On("IScriptPlayer.get_Level()", _ => Int(Me["level"]));
        On("IScriptPlayer.get_Health()", _ => Int(Me["hp"]));
        On("IScriptPlayer.get_MaxHealth()", _ => Int(Me["maxHp"]));
        On("IScriptPlayer.get_Mana()", _ => Int(Me["mp"]));
        On("IScriptPlayer.get_MaxMana()", _ => Int(Me["maxMp"]));
        On("IScriptPlayer.get_Gold()", _ => Int(Me["gold"]));
        On("IScriptPlayer.get_X()", _ => Int(Me["x"]));
        On("IScriptPlayer.get_Y()", _ => Int(Me["y"]));
        On("IScriptPlayer.get_AFK()", _ => (bool?)Me["afk"] ?? false);
        On("IScriptPlayer.get_IsMember()", _ => (bool?)Me["member"] ?? false);
        On("IScriptPlayer.get_Stats()", _ => Models.Stats(Me["stats"]));
        On("IScriptPlayer.get_ID()", _ => Int(Me["id"]));
        On("IScriptPlayer.get_Password()", _ => null);
        On("IScriptPlayer.get_Target()", _ => Me["target"] is JsonObject target ? Models.Monster(target) : null);
        On("IScriptPlayer.get_HasTarget()", _ => Me["target"] is JsonObject target && (bool)target["alive"]!);
        On("IScriptPlayer.get_CurrentClass()", _ => Playing ? InventoryItems().Find(i => i is { Equipped: true, Category: ItemCategory.Class }) : null);
        On("IScriptPlayer.get_ServerIP()", _ => (string?)Aqx.Call("session")?["host"] ?? "");
        On("IScriptPlayer.SetSpawnPoint(String, String)", args => Aqx.Call("respawn", args[0], args[1]));
        On("IScriptPlayer.Goto(String)", args => Act("goto", args[0]));
        On("IScriptPlayer.WalkTo(Int32, Int32, Int32)", args => Act("walk", args[0], args[1]));
        On("IScriptPlayer.Rest(Boolean)", args =>
        {
            Rest((bool)args[0]!);
            return null;
        });
        On("IScriptFaction.get_FactionList()", _ => Me["factions"] is JsonArray factions ? factions.Select(f => Models.Faction(f!)).ToList() : []);
    }

    void Rest(bool full)
    {
        Act("send", "%xt%zm%restRequest%1%%");
        for (int i = 0; full && i < RestPolls && !Rested(Me); i++)
            Sleep(RestPoll);
    }

    static bool Rested(JsonObject me) => Int(me["hp"]) >= Int(me["maxHp"]) && Int(me["mp"]) >= Int(me["maxMp"]);

    bool Playing
    {
        get
        {
            JsonObject me = Me;
            return Phase is "loading" or "ready" && State(me) > 0;
        }
    }

    static int State(JsonObject me) => (string?)me["state"] switch
    {
        "idle" => 1,
        "combat" => 2,
        _ => 0,
    };

    static int Int(JsonNode? value) => value is null ? 0 : (int)Math.Clamp(value.GetValue<double>(), int.MinValue, int.MaxValue);

    List<InventoryItem> InventoryItems() => Models.Items<InventoryItem>(View("inventory"));
}
