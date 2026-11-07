using Newtonsoft.Json.Linq;

namespace Mira.Skua;

static class QuestModels
{
    public static Quest Project(QuestSnapshot quest, string? status)
    {
        static JObject Item(QuestItemSnapshot item) => new() { ["ItemID"] = item.Id, ["sName"] = item.Name, ["iQty"] = item.Qty, ["iStk"] = item.MaxStack, ["bTemp"] = item.Temp };
        static JObject Items(IEnumerable<QuestItemSnapshot> items) => new(items.Select((item, index) => new JProperty(index.ToString(), Item(item))));
        return new JObject
        {
            ["QuestID"] = quest.Id, ["sName"] = quest.Name, ["sDesc"] = quest.Description, ["sEndText"] = quest.EndText, ["iSlot"] = quest.Slot, ["iValue"] = quest.Value,
            ["bOnce"] = quest.Once, ["sField"] = quest.Period?.Field ?? "", ["iIndex"] = quest.Period?.Index ?? 0,
            ["bUpg"] = quest.Member, ["iLvl"] = quest.Level, ["FactionID"] = quest.Faction ?? 0, ["iReqRep"] = quest.RequiredRep,
            ["iClass"] = quest.ClassItem ?? 0, ["iReqCP"] = quest.RequiredCp, ["iGold"] = quest.Gold, ["iExp"] = quest.Exp,
            ["status"] = status switch { "active" => "p", "completable" => "c", _ => null },
            ["oItems"] = Items(quest.TurnIn), ["oReqd"] = Items(quest.Required),
            ["turnin"] = new JArray(quest.TurnIn.Select(i => new JObject { ["ItemID"] = i.Id, ["iQty"] = i.Qty })),
            ["oRewards"] = new JObject { ["itemsS"] = new JObject(quest.Rewards.Select((r, i) => new JProperty(i.ToString(), new JObject { ["ItemID"] = r.Id, ["sName"] = r.Name, ["iQty"] = r.Qty, ["iStk"] = r.MaxStack, ["bTemp"] = r.Temp }))) },
            ["reward"] = new JArray(quest.Rewards.Select(r => new JObject { ["ItemID"] = r.Id, ["iQty"] = r.Qty, ["iRate"] = r.Chance, ["iType"] = r.Kind switch { "rolled" => 1, "chosen" => 2, "random" => 3, _ => 0 } })),
        }.ToObject<Quest>()!;
    }
}
