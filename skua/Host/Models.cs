using System.Text.Json.Nodes;
using Newtonsoft.Json.Linq;
using Skua.Core.Models.Factions;
using Skua.Core.Models.Items;
using Skua.Core.Models.Monsters;

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
        }.ToObject<T>()!;

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

    public static Monster Monster(JsonNode monster) =>
        new JObject
        {
            ["MonMapID"] = (int)monster["mapId"]!,
            ["MonID"] = (int)monster["monId"]!,
            ["strMonName"] = (string?)monster["name"],
            ["strFrame"] = (string?)monster["cell"],
            ["intHP"] = (int)monster["hp"]!,
            ["intHPMax"] = (int)monster["maxHp"]!,
            ["intState"] = (bool)monster["alive"]! ? 1 : 0,
        }.ToObject<Monster>()!;
}
