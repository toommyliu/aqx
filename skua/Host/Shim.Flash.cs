using System.Collections.Concurrent;
using System.Text.Json.Nodes;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace Aqx.Skua;

public sealed partial class Shim
{
    readonly ConcurrentDictionary<string, bool> preferences = new();

    void Flash()
    {
        On("IFlashUtil.Call(String, Object[])", args => FlashCall((string)args[0]!, (object?[])args[1]!));
        On("IFlashUtil.Call`1(String, Object[])", (args, returns) => FromJson(FlashCall((string)args[0]!, (object?[])args[1]!), returns));
        On("IFlashUtil.Call(String, Type, Object[])", args => FromJson(FlashCall((string)args[0]!, (object?[])args[2]!), (Type)args[1]!));
        On("IFlashUtil.add_FlashCall(FlashCallHandler)", _ => null);
        On("IFlashUtil.remove_FlashCall(FlashCallHandler)", _ => null);
        On("IFlashUtil.InitializeFlash()", _ => null);
        On("IFlashUtil.Dispose()", _ => null);
    }

    string? FlashCall(string function, object?[] args) => (function, args) switch
    {
        ("getGameObject" or "getGameObjectS", [string path]) => GameObject(path),
        ("setGameObject" or "setGameObjectKey", _) => null,
        ("callGameFunction0", ["world.toggleMonsters" or "world.toggleQuestLog"]) => null,
        ("callGameFunction0", ["world.myAvatar.isEmailVerified"]) => JsonConvert.SerializeObject((bool?)Me["emailVerified"] ?? false),
        ("callGameFunction", ["world.loadMap" or "world.myAvatar.addItem", _]) => null,
        ("callGameFunction", ["toggleItemEquip", IDictionary<string, object?> item]) => JsonConvert.SerializeObject(Act("equip", Convert.ToInt32(item["ItemID"]))),
        ("callGameFunction", ["world.testAction", IDictionary<string, object?> skill]) => JsonConvert.SerializeObject((bool?)Aqx.Call("cast", Convert.ToInt32(skill["slot"])) ?? false),
        ("callGameFunction", ["world.abandonQuest", int quest]) => JsonConvert.SerializeObject(Aqx.Call("abandon", quest)),
        ("callGameFunction", ["world.maximumQuestTurnIns", int quest]) => JsonConvert.SerializeObject(EnsureLoad(quest) is { } q ? TurnIns(q) : 0),
        ("callGameFunction", ["world.tryQuestComplete", int quest, int reward, bool _, int times]) => JsonConvert.SerializeObject(Complete(quest, reward, times)),
        ("callGameFunction", ["loaderInfo.applicationDomain.hasDefinition", "InvCat"]) => "true",
        ("callGameFunction", ["loaderInfo.applicationDomain.getDefinition", "InvCat"]) =>
            new JObject { ["MISC_SLOTS"] = Int(View("slots")?["misc"]), ["MISC_TYPES"] = new JArray(MiscTypes) }.ToString(Formatting.None),
        ("callGameFunction", ["world.getQuestValue", int slot]) => JsonConvert.SerializeObject(QuestValue(slot)),
        ("callGameFunction", ["world.getAchievement", string field, int index]) => JsonConvert.SerializeObject((bool?)Aqx.Call("achievement", field, index) ?? false),
        ("callGameFunction", ["world.sendLoadShopRequest", int shop]) => JsonConvert.SerializeObject(LoadShop(shop)),
        ("callGameFunction", ["world.sendBuyItemRequestWithQuantity", IDictionary<string, object?> item]) =>
            JsonConvert.SerializeObject(Buy(Convert.ToInt32(item["ItemID"]), Convert.ToInt32(item["ShopItemID"]), Convert.ToInt32(item["iQty"]))),
        ("canUseSkill", [int slot]) => JsonConvert.SerializeObject((bool?)Aqx.Call("can_cast", slot) ?? false),
        ("useSkill", [int slot]) => JsonConvert.SerializeObject((bool?)Aqx.Call("cast", slot) ?? false),
        ("availableMonsters", []) => CellMonsterData().ToString(Formatting.None),
        ("isNull", [string path]) => JsonConvert.SerializeObject(GameObject(path) is null),
        ("getArrayObject", ["world.actions.active", int slot]) => View("skills") is JsonArray skills && slot >= 0 && slot < skills.Count ? skills[slot]?.ToJsonString() : null,
        _ => Missing($"Flash.Call(\"{function}\", {string.Join(", ", args.Select(a => $"{a}"))})"),
    };

    string? GameObject(string path) => path switch
    {
        _ when path.StartsWith("uoPref.") => JsonConvert.SerializeObject(preferences.GetValueOrDefault(path["uoPref.".Length..], true)),
        _ when path.StartsWith("world.lock.") => "{\"TS\":0,\"CD\":0}",
        _ when path.StartsWith("world.invTree.") => InventoryTree(path["world.invTree.".Length..]),
        _ when path == "world.shopinfo" || path.StartsWith("world.shopinfo.") => Walk(ShopInfo(), path["world.shopinfo".Length..])?.ToJsonString(),
        "world.myAvatar.objData.strEmail" or "world.myAvatar.objData.dCreated" or "world.myAvatar.objData.strGender" => null,
        "ui.mcPopup.currentLabel" => "\"\"",
        "ui.monsterIcon.redX.visible" => "false",
        "world.myAvatar.objData.guild.Name" => "\"\"",
        "world.myAvatar.objData.CharID" => JsonConvert.SerializeObject(Int(Me["charId"])),
        "world.myAvatar.objData.intAQ" => JsonConvert.SerializeObject((bool?)Me["aqLinked"] == true ? 1 : 0),
        "sfc.myUserName" => JsonConvert.SerializeObject((string?)Me["name"] ?? ""),
        "world.myAvatar.objData.iUpgDays" => JsonConvert.SerializeObject((bool?)Me["member"] == true ? 0 : -1),
        "world.myAvatar.objData.intLevel" => JsonConvert.SerializeObject(Int(Me["level"])),
        "world.myAvatar.objData.intGold" => JsonConvert.SerializeObject(Int(Me["gold"])),
        "world.myAvatar.objData.intCoins" => JsonConvert.SerializeObject(Int(Me["coins"])),
        "world.myAvatar.objData.iBoostCP" => JsonConvert.SerializeObject(Int(Me["boosts"]?["class"])),
        "world.myAvatar.objData.iBoostXP" => JsonConvert.SerializeObject(Int(Me["boosts"]?["experience"])),
        "world.myAvatar.objData.iBoostG" => JsonConvert.SerializeObject(Int(Me["boosts"]?["gold"])),
        "world.myAvatar.objData.iBoostRep" => JsonConvert.SerializeObject(Int(Me["boosts"]?["reputation"])),
        "world.myAvatar.objData.strClassName" => JsonConvert.SerializeObject((string?)Me["class"] ?? ""),
        "world.strMapName" or "world.strMapFileName" => JsonConvert.SerializeObject(MapName),
        "world.myAvatar.items" => Models.ItemArray(View("inventory")).ToString(Formatting.None),
        "world.myAvatar.houseitems" => Models.ItemArray(View("house")).ToString(Formatting.None),
        "world.bankinfo.items" => Models.ItemArray(View("bank")).ToString(Formatting.None),
        _ => Missing($"Flash game object {path}"),
    };

    string? InventoryTree(string path)
    {
        string[] parts = path.Split('.', 2);
        JsonNode? item = (View("inventory") as JsonArray)?.FirstOrDefault(i => parts[0] == $"{i?["id"]}");
        if (item is null || parts.Length < 2)
            return null;
        JsonNode? enhancement = item["enhancement"];
        return parts[1] switch
        {
            "iEnh" or "EnhID" => JsonConvert.SerializeObject((int?)enhancement?["id"] ?? 0),
            "EnhLvl" => JsonConvert.SerializeObject((int?)enhancement?["level"] ?? 0),
            "ProcID" => null,
            "sLink" or "sFile" or "sIcon" or "sES" or "sMeta" => JsonConvert.SerializeObject((string?)item[TreeFields[parts[1]]] ?? ""),
            _ => Missing($"Flash game object world.invTree.{path}"),
        };
    }

    static readonly string[] MiscTypes = ["Item", "Note", "Quest Item", "Resource"];

    static readonly Dictionary<string, string> TreeFields = new() { ["sLink"] = "link", ["sFile"] = "file", ["sIcon"] = "icon", ["sES"] = "slot", ["sMeta"] = "meta" };

    static JsonNode? Walk(JsonNode? node, string path) =>
        path.Split('.', StringSplitOptions.RemoveEmptyEntries).Aggregate(node, (at, key) => at switch
        {
            JsonObject o => o[key],
            JsonArray a when int.TryParse(key, out int i) && i >= 0 && i < a.Count => a[i],
            _ => null,
        });

    string? Missing(string what)
    {
        Aqx.Call("log", "warn", $"{what} is not implemented");
        throw new NotImplementedException($"{what} is not implemented");
    }

    static object? FromJson(string? json, Type type) =>
        json is null ? (type.IsValueType ? Activator.CreateInstance(type) : null) : type == typeof(string) ? json : JsonConvert.DeserializeObject(json, type);

    void Remember(string packet)
    {
        if (packet.Split('%') is [_, "xt", "zm", "cmd", _, "uopref", string key, string value, ..] && bool.TryParse(value, out bool on))
            preferences[key] = on;
    }
}
