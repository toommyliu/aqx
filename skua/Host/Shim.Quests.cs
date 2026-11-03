using System.Text.Json.Nodes;
using Newtonsoft.Json.Linq;
using Skua.Core.Models.Items;
using Skua.Core.Models.Quests;

namespace Aqx.Skua;

public sealed partial class Shim
{
    readonly Dictionary<int, int> registered = [];
    readonly Dictionary<int, long> lastAccept = [];
    readonly Dictionary<int, long> lastComplete = [];
    int registerInterval = 2000;
    bool registeredLoop;

    void Quests()
    {
        On("IScriptQuest.get_Tree()", _ => QuestTree());
        On("IScriptQuest.get_Active()", _ => QuestTree().FindAll(q => q.Active));
        On("IScriptQuest.get_Completed()", _ => QuestTree().FindAll(q => q.Status == "c"));
        On("IScriptQuest.Load(Int32[])", args => Act("load_quests", (int[])args[0]!));
        On("IScriptQuest.EnsureLoad(Int32)", args => EnsureLoad((int)args[0]!));
        On("IScriptQuest.TryGetQuest(Int32, Quest&)", args => (args[1] = FindQuest((int)args[0]!)) is not null);
        On("IScriptQuest.Accept(Int32)", args => Accept((int)args[0]!));
        On("IScriptQuest.Accept(Int32[])", args => Each((int[])args[0]!, id => Accept(id)));
        On("IScriptQuest.EnsureAccept(Int32)", args => EnsureAccept((int)args[0]!));
        On("IScriptQuest.EnsureAccept(Int32[])", args => Each((int[])args[0]!, id => EnsureAccept(id)));
        On("IScriptQuest.Complete(Int32, Int32, Boolean)", args => Complete((int)args[0]!, (int)args[1]!, 1));
        On("IScriptQuest.Complete(Int32[])", args => Each((int[])args[0]!, id => Complete(id, -1, 1)));
        On("IScriptQuest.EnsureComplete(Int32, Int32, Boolean)", args => EnsureComplete((int)args[0]!, (int)args[1]!));
        On("IScriptQuest.EnsureComplete(Int32[])", args => Each((int[])args[0]!, id => EnsureComplete(id, -1)));
        On("IScriptQuest.CanComplete(Int32)", args => CanComplete((int)args[0]!));
        On("IScriptQuest.CanCompleteFullCheck(Int32)", args =>
        {
            int id = (int)args[0]!;
            if (CanComplete(id))
                return true;
            return EnsureLoad(id) is { } quest && quest.Requirements.Concat(quest.AcceptRequirements).All(i => Held(i.ID) >= i.Quantity);
        });
        On("IScriptQuest.IsInProgress(Int32)", args => InProgress((int)args[0]!));
        On("IScriptQuest.IsAvailable(Int32)", args => EnsureLoad((int)args[0]!) is { } quest && IsAvailable(quest));
        On("IScriptQuest.IsUnlocked(Int32)", args => EnsureLoad((int)args[0]!) is { } quest && IsUnlocked(quest));
        On("IScriptQuest.IsUnlocked(Quest)", args => IsUnlocked((Quest)args[0]!));
        On("IScriptQuest.HasBeenCompleted(Int32)", args => EnsureLoad((int)args[0]!) is { } quest && HasBeenCompleted(quest));
        On("IScriptQuest.HasBeenCompleted(Quest)", args => HasBeenCompleted((Quest)args[0]!));
        On("IScriptQuest.IsDailyComplete(Int32)", args => EnsureLoad((int)args[0]!) is { } quest && IsDailyComplete(quest));
        On("IScriptQuest.IsDailyComplete(Quest)", args => IsDailyComplete((Quest)args[0]!));
        On("IScriptQuest.UpdateQuest(Int32)", args => EnsureLoad((int)args[0]!) is not null);
        On("IScriptQuest.UpdateQuest(Int32, Int32)", _ => null);
        On("IScriptQuest.get_RegisterCompleteInterval()", _ => registerInterval);
        On("IScriptQuest.set_RegisterCompleteInterval(Int32)", args => registerInterval = (int)args[0]!);
        On("IScriptQuest.get_Registered()", _ => Locked(registered, () => registered.Keys.ToList()));
        On("IScriptQuest.get_RegisteredRewards()", _ => Locked(registered, () => new Dictionary<int, int>(registered)));
        On("IScriptQuest.RegisterQuests(Int32[])", args => Register(((int[])args[0]!).Select(id => (id, -1))));
        On("IScriptQuest.RegisterQuests(ValueTuple`2[])", args => Register((ValueTuple<int, int>[])args[0]!));
        On("IScriptQuest.UnregisterQuests(Int32[])", args => Locked(registered, () => Array.ForEach((int[])args[0]!, id => registered.Remove(id))));
        On("IScriptQuest.UnregisterAllQuests()", _ => Locked(registered, registered.Clear));
        On("IScriptQuest.add_PropertyChanged(PropertyChangedEventHandler)", _ => null);
        On("IScriptQuest.remove_PropertyChanged(PropertyChangedEventHandler)", _ => null);
    }

    List<Quest> QuestTree() => Aqx.Call("quests") is JsonArray quests ? quests.Select(q => QuestModel(q!)).ToList() : [];

    Quest? FindQuest(int id) => Aqx.Call("quest", id) is JsonObject quest ? QuestModel(quest) : null;

    Quest? EnsureLoad(int id) => FindQuest(id) ?? (Act("load_quests", new[] { id }) ? FindQuest(id) : null);

    bool InProgress(int id) => (string?)Aqx.Call("quest", id)?["status"] is "active" or "completable";

    bool CanComplete(int id) => (string?)Aqx.Call("quest", id)?["status"] == "completable";

    bool Accept(int id) => Act("accept", id) && InProgress(id);

    bool EnsureAccept(int id)
    {
        for (int i = 0; i < SettingInt("QuestAcceptAndCompleteTries") && !Accept(id); i++)
            Sleep(SettingInt("ActionDelay"));
        return InProgress(id);
    }

    bool Complete(int id, int reward, int times) => Act("complete", id, reward > 0 ? reward : null, times);

    bool EnsureComplete(int id, int reward)
    {
        if (id == 0)
            return true;
        for (int i = 0; i < SettingInt("QuestAcceptAndCompleteTries") && InProgress(id); i++)
            if (!Complete(id, reward, 1))
                Sleep(SettingInt("ActionDelay"));
        return !InProgress(id);
    }

    object? Each(int[] ids, Action<int> act)
    {
        foreach (int id in ids)
        {
            act(id);
            Sleep(SettingInt("ActionDelay"));
        }
        return null;
    }

    bool IsUnlocked(Quest quest) => quest.Slot < 0 || QuestValue(quest.Slot) >= quest.Value - 1;

    bool HasBeenCompleted(Quest quest) => quest.Slot < 0 || QuestValue(quest.Slot) >= quest.Value;

    bool IsDailyComplete(Quest quest) => (bool?)Aqx.Call("achievement", quest.Field ?? "", quest.Index) ?? false;

    bool IsAvailable(Quest quest)
    {
        JsonObject me = Me;
        int rep = me["factions"] is JsonArray factions ? factions.Where(f => Int(f?["id"]) == quest.RequiredFactionId).Select(f => Int(f?["rep"])).FirstOrDefault() : 0;
        return !IsDailyComplete(quest)
            && IsUnlocked(quest)
            && (!quest.Upgrade || ((bool?)me["member"] ?? false))
            && Int(me["level"]) >= quest.Level
            && (quest.RequiredFactionId <= 1 || rep >= quest.RequiredFactionRep);
    }

    int TurnIns(Quest quest) => quest.Requirements.Select(r => r.Quantity > 0 ? Held(r.ID) / r.Quantity : int.MaxValue).DefaultIfEmpty(1).Min();

    int QuestValue(int slot) => Int(Aqx.Call("quest_value", slot));

    int SettingInt(string name) => settings.GetValueOrDefault($"IScriptOption.{name}") as int? ?? (int)SkuaDefaults[$"IScriptOption.{name}"];

    int Held(int id) => InventoryItems().Concat(Models.Items<ItemBase>(View("temp"))).Where(i => i.ID == id).Sum(i => i.Quantity);

    static Quest QuestModel(JsonNode quest)
    {
        static JObject Item(JsonNode? item) => new()
        {
            ["ItemID"] = (int?)item?["item"] ?? 0,
            ["sName"] = (string?)item?["name"],
            ["iQty"] = (int?)item?["qty"] ?? 0,
            ["iStk"] = (int?)item?["maxStack"] ?? 1,
        };
        static JObject ByPosition(JsonNode? items) => items is JsonArray list ? new JObject(list.Select((item, at) => new JProperty($"{at}", Item(item)))) : [];
        JArray turnIn = quest["requirements"] is JsonArray needs ? new(needs.Select(n => new JObject { ["ItemID"] = (int?)n?["item"] ?? 0, ["iQty"] = (int?)n?["qty"] ?? 0 })) : [];
        return new JObject
        {
            ["QuestID"] = (int?)quest["id"] ?? 0,
            ["sName"] = (string?)quest["name"],
            ["iSlot"] = (int?)quest["slot"] ?? -1,
            ["iValue"] = (int?)quest["value"] ?? 0,
            ["bOnce"] = (bool?)quest["once"] ?? false,
            ["sField"] = (string?)quest["field"] ?? "",
            ["iIndex"] = (int?)quest["index"] ?? 0,
            ["bUpg"] = (bool?)quest["member"] ?? false,
            ["iLvl"] = (int?)quest["level"] ?? 0,
            ["FactionID"] = (int?)quest["faction"] ?? 0,
            ["iReqRep"] = (int?)quest["rep"] ?? 0,
            ["status"] = (string?)quest["status"] switch
            {
                "active" => "p",
                "completable" => "c",
                _ => null,
            },
            ["oItems"] = ByPosition(quest["requirements"]),
            ["turnin"] = turnIn,
            ["oReqd"] = ByPosition(quest["held"]),
            ["oRewards"] = new JObject { ["itemsS"] = ByPosition(quest["rewards"]) },
            ["reward"] = quest["rewards"] is JsonArray rewards ? new JArray(rewards.Select(r => new JObject(Item(r))
            {
                ["iRate"] = (double?)r?["chance"] ?? 0,
                ["iType"] = (string?)r?["kind"] switch { "roll" => 1, "choice" => 2, "random" => 3, _ => 0 },
            })) : new JArray(),
        }.ToObject<Quest>()!;
    }

    object? Register(IEnumerable<(int Id, int Reward)> quests)
    {
        lock (registered)
        {
            foreach ((int id, int reward) in quests)
                registered[id] = reward;
            if (!registeredLoop)
            {
                registeredLoop = true;
                Background("Skua registered quests", CompleteRegistered);
            }
        }
        return null;
    }

    void CompleteRegistered()
    {
        while (!Aqx.Stopping.IsCancellationRequested)
        {
            Thread.Sleep(registerInterval);
            if (!Playing)
                continue;
            foreach ((int id, int reward) in Locked(registered, () => registered.ToList()))
            {
                long now = Environment.TickCount64;
                if (now - lastComplete.GetValueOrDefault(id, long.MinValue / 2) < 3000)
                    continue;
                if (!InProgress(id))
                {
                    if (now - lastAccept.GetValueOrDefault(id, long.MinValue / 2) < 2000)
                        continue;
                    Accept(id);
                    lastAccept[id] = Environment.TickCount64;
                }
                if (!CanComplete(id) || FindQuest(id) is not { } quest)
                    continue;
                int times = quest.Once || !string.IsNullOrEmpty(quest.Field) ? 1 : Math.Max(1, TurnIns(quest));
                Complete(id, reward, times);
                lastComplete[id] = Environment.TickCount64;
                if (settings.GetValueOrDefault("IScriptLite.ReacceptQuest") is not true)
                {
                    Accept(id);
                    lastAccept[id] = Environment.TickCount64;
                }
                break;
            }
        }
    }
}
