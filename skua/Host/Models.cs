using System.Text.Json.Nodes;
using Newtonsoft.Json.Linq;
using Skua.Core.Models.Factions;
using Skua.Core.Models.Items;
using Skua.Core.Models.Monsters;
using Skua.Core.Models.Players;

namespace Aqx.Skua;

public static class Models
{
    public static List<T> Items<T>(JsonNode? items, bool temp = false) where T : ItemBase =>
        items is JsonArray list ? list.Select(i => Item<T>(i!, temp)).ToList() : [];

    public static T Item<T>(JsonNode item, bool temp = false) where T : ItemBase =>
        new JObject
        {
            ["ItemID"] = (int?)item["id"] ?? 0,
            ["sName"] = (string?)item["name"],
            ["iQty"] = (int?)item["qty"] ?? 0,
            ["iStk"] = (int?)item["maxStack"] ?? 1,
            ["sType"] = (string?)item["type"] ?? "",
            ["bEquip"] = (bool?)item["equipped"] ?? false,
            ["bCoins"] = (bool?)item["ac"] ?? false,
            ["bUpg"] = (bool?)item["member"] ?? false,
            ["bTemp"] = temp,
            ["EnhLvl"] = (int?)item["enhancement"]?["level"] ?? 0,
        }.ToObject<T>()!;

    static readonly (string Skua, string Aqx)[] StatNames =
    [
        ("$STR", "str"), ("$INT", "int"), ("$DEX", "dex"), ("$WIS", "wis"), ("$END", "end"), ("$LCK", "luk"),
        ("$ap", "attackPower"), ("$sp", "spellPower"), ("$tha", "haste"), ("$tcr", "crit"), ("$scm", "critMultiplier"), ("$tdo", "dodge"),
    ];

    public static PlayerStats Stats(JsonNode? stats) =>
        new JObject(StatNames.Select(n => new JProperty(n.Skua, (double?)stats?[n.Aqx] ?? 0))).ToObject<PlayerStats>()!;

    public static PlayerInfo Player(JsonNode player) =>
        new JObject
        {
            ["entID"] = (int?)player["id"] ?? 0,
            ["uoName"] = (string?)player["name"] ?? "",
            ["intLevel"] = (int?)player["level"] ?? 0,
            ["intHP"] = (int?)player["hp"] ?? 0,
            ["intHPMax"] = (int?)player["maxHp"] ?? 0,
            ["intMP"] = (int?)player["mp"] ?? 0,
            ["strFrame"] = (string?)player["cell"] ?? "",
            ["strPad"] = (string?)player["pad"] ?? "",
            ["afk"] = (bool?)player["afk"] ?? false,
        }.ToObject<PlayerInfo>()!;

    public static Faction Faction(JsonNode faction) =>
        new JObject
        {
            ["FactionID"] = (int)faction["id"]!,
            ["sName"] = (string?)faction["name"],
            ["iRank"] = (int)faction["rank"]!,
            ["iRep"] = (int)faction["rep"]!,
            ["iSpillRep"] = (int)faction["repInRank"]!,
            ["iRepToRank"] = (int)faction["repForNextRank"]!,
        }.ToObject<Faction>()!;

    public static List<Monster> Monsters(JsonNode? monsters) =>
        monsters is JsonArray list ? list.Select(m => Monster(m!)).ToList() : [];

    public static Monster Monster(JsonNode monster) => MonsterData(monster).ToObject<Monster>()!;

    public static JObject MonsterData(JsonNode monster) =>
        new JObject
        {
            ["MonMapID"] = (int)monster["mapId"]!,
            ["MonID"] = (int)monster["monId"]!,
            ["strMonName"] = (string?)monster["name"],
            ["strFrame"] = (string?)monster["cell"],
            ["intHP"] = (int)monster["hp"]!,
            ["intHPMax"] = (int)monster["maxHp"]!,
            ["intState"] = (string?)monster["state"] switch { "combat" => 2, "idle" => 1, _ => 0 },
        };
}
