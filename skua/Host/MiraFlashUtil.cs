using System.Collections.Concurrent;
using Newtonsoft.Json;
using Skua.Core.Flash;

namespace Mira.Skua;

public sealed class MiraFlashUtil(Protocol protocol, State state) : IFlashUtil
{
    readonly ConcurrentDictionary<string, bool> preferences = new();
    System.Collections.Immutable.ImmutableDictionary<long, System.Collections.Immutable.ImmutableHashSet<int>> favorites = System.Collections.Immutable.ImmutableDictionary<long, System.Collections.Immutable.ImmutableHashSet<int>>.Empty;
    public event FlashCallHandler? FlashCall;
    public void InitializeFlash() { }
    public void Dispose() { }
    public string? Call(string function, params object[] args)
    {
        object? value = Read(function, args);
        return value is null ? null : JsonConvert.SerializeObject(value);
    }
    public T? Call<T>(string function, params object[] args) => (T?)Call(function, typeof(T), args);
    public object? Call(string function, Type type, params object[] args)
    {
        string? json = Call(function, args);
        return type == typeof(string) ? json : json is null ? (type.IsValueType ? Activator.CreateInstance(type) : null) : JsonConvert.DeserializeObject(json, type);
    }
    [Unsupported] public object FromFlashXml(XElement el) => throw new NotSupportedException("IFlashUtil.FromFlashXml");
    public IFlashObject<T> CreateFlashObject<T>(string path) => path == "world.actions.active"
        ? new FlashObject<T>(Link(new SkillArray()), this)
        : throw new NotSupportedException($"IFlashUtil.CreateFlashObject {path}");
    abstract record SkillLink;
    sealed record SkillArray() : SkillLink;
    sealed record SkillEntry(int Index) : SkillLink;
    sealed record SkillTime(int Index) : SkillLink;
    sealed record SkillLength() : SkillLink;
    readonly ConcurrentDictionary<int, SkillLink> skillLinks = new();
    int nextLink;
    int Link(SkillLink link) { int id = Interlocked.Increment(ref nextLink); skillLinks[id] = link; return id; }
    object LinkValue(int id) => skillLinks[id] switch
    {
        SkillArray => IScriptInterface.Instance.Player.Skills ?? [],
        SkillEntry entry => ActiveSkill(entry.Index)!,
        SkillTime time => Slot(time.Index) is { } slot ? (long)(state.Combat.SkillTiming?.GetValueOrDefault(slot)?.LastUse ?? 0) : 0L,
        SkillLength => state.Me?.Skills.Length ?? 0,
        _ => throw new NotSupportedException("IFlashUtil skill link")
    };
    int ChildLink(int id, string path) => (skillLinks[id], path) switch
    {
        (SkillArray, "length") => Link(new SkillLength()),
        (SkillEntry entry, "ts") => Link(new SkillTime(entry.Index)),
        _ => throw new NotSupportedException($"IFlashUtil skill child {path}")
    };
    object? SetLinkValue(int id, object value)
    {
        if (skillLinks[id] is not SkillTime time) throw new NotSupportedException("IFlashUtil skill value");
        long at = Convert.ToInt64(value);
        if (Slot(time.Index) is { } slot) protocol.Call(new SetSkillLastUseCall(slot, at));
        return null;
    }
    object? DestroyLink(int id) { skillLinks.TryRemove(id, out _); return null; }
    object? Read(string function, object[] args) => (function, args) switch
    {
        ("lnkGetValue", [int id]) => LinkValue(id),
        ("lnkGetChild", [int id, string path]) => ChildLink(id, path),
        ("lnkGetArray", [int id, int index]) => Link(new SkillEntry(index)),
        ("lnkSetValue", [int id, object value]) => SetLinkValue(id, value),
        ("lnkDestroy", [int id]) => DestroyLink(id),
        ("getGameObject" or "getGameObjectS", [string path]) => GameObject(path),
        ("setGameObject", ["litePreference.data.favs", object value]) => SetFavorites(value),
        ("setGameObject" or "setGameObjectKey", _) => null,
        ("isFavoriteItem", [int id]) => Volatile.Read(ref favorites).GetValueOrDefault(state.Me?.CharId ?? 0)?.Contains(id) == true,
        ("rejectExcept", [string names]) => ((MiraDrop)IScriptInterface.Instance.Drops).RejectOffers(names.Split(',')),
        ("getUnmetPurchaseRequirements", [int id, int listing, int quantity]) => IScriptInterface.Instance.Shops.GetUnmetPurchaseRequirements(new ShopItem { ID = id, ShopItemID = listing }, quantity),
        ("inventoryHasSpaceFor", [string item, int quantity, bool purchase]) => InventorySpace(item, quantity, purchase, null),
        ("inventoryHasSpaceFor", [string item, int quantity, bool purchase, string outgoing]) => InventorySpace(item, quantity, purchase, outgoing),
        ("callGameFunction0", ["world.toggleMonsters" or "world.toggleQuestLog"]) => null,
        ("callGameFunction0", ["world.myAvatar.isEmailVerified"]) => state.Me?.EmailVerified ?? false,
        ("callGameFunction", ["sfc.sendString" or "sfc.sendJson" or "sfc.sendXml", string packet]) => SendPacket(packet),
        ("callGameFunction", ["world.loadMap" or "world.myAvatar.addItem", _]) => null,
        ("callGameFunction", ["toggleItemEquip", IDictionary<string, object?> item]) => Act(new EquipCall(Convert.ToInt64(item["ItemID"]))),
        ("callGameFunction", ["world.testAction", IDictionary<string, object?> skill]) => UseSkill(Convert.ToInt32(skill["slot"])),
        ("callGameFunction", ["world.abandonQuest", int quest]) => Act(new AbandonCall(quest)),
        ("callGameFunction", ["world.maximumQuestTurnIns", int quest]) => MaximumTurnIns(quest),
        ("callGameFunction", ["world.showQuests", string ids, string _]) => LoadQuests(ids),
        ("callGameFunction", ["world.acceptQuest", int quest]) => IScriptInterface.Instance.Quests.Accept(quest),
        ("callGameFunction", ["world.isQuestInProgress", int quest]) => IScriptInterface.Instance.Quests.IsInProgress(quest),
        ("callGameFunction", ["world.tryQuestComplete", int quest, int reward, bool special]) => IScriptInterface.Instance.Quests.Complete(quest, reward, special),
        ("callGameFunction", ["world.tryQuestComplete", int quest, int reward, bool special, int times]) => Act(new CompleteCall(quest, reward < 0 ? null : reward, times, special)),
        ("callGameFunction", ["loaderInfo.applicationDomain.hasDefinition", "InvCat"]) => true,
        ("callGameFunction", ["loaderInfo.applicationDomain.getDefinition", "InvCat"]) => new { MISC_SLOTS = state.Me?.BagSlots ?? 0, MISC_TYPES = new[] { "Item", "Note", "Quest Item", "Resource" } },
        ("callGameFunction", ["world.myAvatar.getCPByID", int id]) => state.Inventory.ById.GetValueOrDefault(id)?.Qty ?? -1,
        ("callGameFunction", ["world.myAvatar.getRep", int id]) => state.Factions.FirstOrDefault(f => f.Id == id)?.Rep ?? 0,
        ("callGameFunction", ["world.getQuestValue", int slot]) => QuestValue(slot),
        ("callGameFunction", ["world.getAchievement", string field, int index]) => Achievement(field, index),
        ("callGameFunction", ["world.maximumShopBuys", object item]) => MaximumShopBuys(item),
        ("callGameFunction", ["world.sendLoadShopRequest", int shop]) => Act(new LoadShopCall(shop)),
        ("callGameFunction", ["world.sendBuyItemRequestWithQuantity", IDictionary<string, object?> item]) => Act(new BuyCall(Convert.ToInt64(item["ShopItemID"]), Convert.ToUInt32(item["iQty"]))),
        ("canUseSkill", [int slot]) => CanUseSkill(slot),
        ("canUseSkill", [int slot, bool ignoreTarget]) => CanUseSkill(slot, ignoreTarget),
        ("useSkill", [int slot]) => UseSkill(slot),
        ("skillCooldownRemaining", [int slot]) => Cooldown(slot),
        ("globalCooldownRemaining", []) => Remaining(state.Combat.GlobalCooldown),
        ("availableMonsters", []) => state.Room?.Monsters.Where(m => m.Cell == state.Room.Cell).Select(m => m.ToScript()),
        ("isNull", [string path]) => GameObject(path) is null,
        ("getArrayObject", ["world.actions.active", int slot]) => ActiveSkill(slot),
        _ => throw new NotSupportedException($"IFlashUtil.Call {function}")
    };
    object? GameObject(string path) => path switch
    {
        _ when path.StartsWith("uoPref.") => preferences.GetValueOrDefault(path[7..], true),
        _ when path.StartsWith("world.lock.") => state.Combat.ActionLocks?.GetValueOrDefault(path[11..]) is { } action ? new { TS = action.Used ?? 0, CD = action.Cooldown } : null,
        _ when path.StartsWith("world.invTree.") => InventoryTree(path[14..]),
        _ when path == "world.shopinfo" || path.StartsWith("world.shopinfo.") => ShopObject(path[14..].TrimStart('.')),
        "world.myAvatar.objData.strEmail" or "world.myAvatar.objData.dCreated" or "world.myAvatar.objData.strGender" => null,
        "ui.mcPopup.currentLabel" => "",
        "ui.monsterIcon.redX.visible" => false,
        "world.myAvatar.objData.guild.Name" => "",
        "world.myAvatar.objData.CharID" => state.Me?.CharId,
        "world.myAvatar.objData.intAQ" => state.Me?.Aq ?? 0,
        "sfc.myUserName" => state.Me?.Name.ToLowerInvariant() ?? "",
        "world.myAvatar.objData.iUpgDays" => state.Me?.Member == true ? 0 : -1,
        "world.myAvatar.objData.intLevel" => state.Me?.Level,
        "world.myAvatar.objData.intGold" => state.Me?.Gold,
        "world.myAvatar.objData.intCoins" => state.Me?.Coins,
        "world.myAvatar.objData.iBoostCP" => Boost("class_points"),
        "world.myAvatar.objData.iBoostXP" => Boost("xp"),
        "world.myAvatar.objData.iBoostG" => Boost("gold"),
        "world.myAvatar.objData.iBoostRep" => Boost("rep"),
        "world.myAvatar.objData.strClassName" => state.Me?.ClassName ?? "",
        "world.strMapName" or "world.strMapFileName" => state.Room?.MapName ?? "",
        "world.questTree" => state.Quests.ScriptTree.ToDictionary(q => q.ID),
        "cDropsUI.invTree" => ((MiraDrop)IScriptInterface.Instance.Drops).VisibleOffers,
        "world.myAvatar.items" => state.Inventory.InventoryItems,
        "world.myAvatar.houseitems" => state.Inventory.HouseItems,
        "world.bankinfo.items" => state.Bank?.ScriptItems,
        _ => throw new NotSupportedException($"IFlashUtil game object {path}")
    };
    object? ShopObject(string path)
    {
        if (state.Shop is not { } shop) return null;
        return path switch
        {
            "" => new { ShopID = shop.Id, sName = shop.Name, bUpg = shop.Member, bLimited = shop.Limited, sMap = string.Join(',', shop.Maps), iIndex = shop.Index, sField = shop.Field, reqItems = shop.RequiredItem, items = shop.ScriptItems },
            "iIndex" => shop.Index,
            "sField" => shop.Field,
            "reqItems" => shop.RequiredItem,
            "ShopID" => shop.Id,
            "sName" => shop.Name,
            "bUpg" => shop.Member,
            "bLimited" => shop.Limited,
            "sMap" => string.Join(',', shop.Maps),
            "items" => shop.ScriptItems,
            _ => null
        };
    }
    int MaximumShopBuys(object item)
    {
        var value = Newtonsoft.Json.Linq.JObject.FromObject(item);
        return ((MiraShop)IScriptInterface.Instance.Shops).MaximumBuys(value.ToObject<ShopItem>()!, value["turnin"] is { Type: not Newtonsoft.Json.Linq.JTokenType.Null });
    }
    object? SetFavorites(object value)
    {
        var parsed = JsonConvert.DeserializeObject<Dictionary<long, int[]>>(JsonConvert.SerializeObject(value))!;
        Volatile.Write(ref favorites, System.Collections.Immutable.ImmutableDictionary.ToImmutableDictionary(parsed, p => p.Key, p => System.Collections.Immutable.ImmutableHashSet.ToImmutableHashSet(p.Value)));
        return null;
    }
    bool InventorySpace(string item, int quantity, bool purchase, string? outgoing)
    {
        ItemBase parsed = purchase ? JsonConvert.DeserializeObject<ShopItem>(item)! : JsonConvert.DeserializeObject<InventoryItem>(item)!;
        return ((MiraInventory)IScriptInterface.Instance.Inventory).HasSpaceFor(parsed, quantity, outgoing is null ? null : JsonConvert.DeserializeObject<InventoryItem>(outgoing));
    }
    object? InventoryTree(string path)
    {
        string[] parts = path.Split('.', 2);
        if (!long.TryParse(parts[0], out long id) || !state.Inventory.ById.TryGetValue(id, out var item) || parts.Length < 2) return null;
        return parts[1] switch
        {
            "sDesc" => item.Details.Description,
            "iLvl" => item.Details.Level,
            "iCost" => item.Details.Cost,
            "iHrs" => item.Details.Hours,
            "iQSindex" => item.Details.QuestSlot,
            "iQSvalue" => item.Details.QuestValue,
            "sQuest" => item.Details.Quest,
            "iEnh" or "EnhID" => item.Enhancement?.Id ?? 0,
            "EnhLvl" => item.Enhancement?.Level ?? 0,
            "ProcID" => item.Enhancement?.ProcId ?? 0,
            "sLink" => item.Link,
            "sFile" => item.File,
            "sIcon" => "",
            "sES" => item.Slot,
            "sMeta" => item.Meta,
            _ => throw new NotSupportedException($"IFlashUtil game object world.invTree.{path}")
        };
    }
    int Boost(string kind) => state.Me?.Boosts.GetValueOrDefault(kind) is { } at ? (int)Math.Max(0, ((long)at - DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()) / 1000) : 0;
    object? ActiveSkill(int slot)
    {
        if (slot < 0 || state.Me is not { } me || slot >= me.Skills.Length) return null;
        var skill = me.Skills[slot];
        var action = Newtonsoft.Json.Linq.JObject.FromObject(skill.ToScript((long)(state.Combat.SkillTiming?.GetValueOrDefault(skill.Slot)?.LastUse ?? 0)));
        action["slot"] = slot;
        action["ref"] = skill.Slot;
        action["tgt"] = skill.Target switch { "self" => "s", "friendly" => "f", _ => "h" };
        action["skillLock"] = skill.SkillLock;
        return action;
    }
    string? Slot(int index) => state.Me?.Skills.ElementAtOrDefault(index)?.Slot;
    bool CanUseSkill(int index, bool ignoreTarget = false)
    {
        if (state.Me is not { } me || index < 0 || index >= me.Skills.Length) return false;
        var skill = me.Skills[index];
        var readiness = state.Combat.Readiness.GetValueOrDefault(skill.Slot);
        if (readiness is null || (readiness.Kind != "ready" && !(readiness.Kind == "at" && Remaining(readiness.At) == 0))) return false;
        if (skill.Slot == "i1" && me.ItemSlot is null) return false;
        if (!ignoreTarget && skill.Target is not ("self" or "friendly") && !(((MiraCombat)IScriptInterface.Instance.Combat).SelectedTarget is { Kind: "monster" } target && state.Room?.MonstersById.GetValueOrDefault(target.Id)?.Hp > 0)) return false;
        return true;
    }
    static int Remaining(ulong? at) => at is { } deadline ? (int)Math.Clamp((long)deadline - DateTimeOffset.UtcNow.ToUnixTimeMilliseconds(), 0, int.MaxValue) : 0;
    int Cooldown(int index) => Slot(index) is { } slot ? Remaining(state.Combat.SkillTiming?.GetValueOrDefault(slot)?.Cooldown) : 0;
    bool UseSkill(int index) => Slot(index) is { } slot && CanUseSkill(index, true) && Act(new UseSkillCall(slot, ((MiraCombat)IScriptInterface.Instance.Combat).SelectedTarget));
    bool Act(Request request) { try { protocol.Call(request); return true; } catch (InvalidOperationException) { return false; } }
    int QuestValue(int slot)
    {
        var strings = state.Me?.Quests.Strings;
        int group = slot / 100, index = slot % 100;
        if (slot < 0 || strings is null || group >= strings.Value.Length || index < 0 || strings.Value[group] is not { } text || index >= text.Length) return -1;
        char value = char.ToUpperInvariant(text[index]);
        return value <= '9' ? value - '0' : value - 'A' + 10;
    }
    int Achievement(string field, int index) => index is < 0 or > 31 || state.Me?.Quests.Achievements.TryGetValue(field, out _) != true ? -1 : (int)((state.Me.Quests.Achievements[field] >> index) & 1);
    int MaximumTurnIns(int quest)
    {
        if (!state.Quests.Definitions.TryGetValue(quest, out var definition)) { IScriptInterface.Instance.Quests.EnsureLoad(quest); definition = state.Quests.Definitions.GetValueOrDefault(quest); }
        if (definition is null) return 0;
        if (definition.TurnIn.IsEmpty) return 1;
        if (definition.Once || definition.Period is not null) return 1;
        return definition.TurnIn.Aggregate(250, (count, item) => Math.Min(count, ((state.Inventory.ById.GetValueOrDefault(item.Id)?.Qty ?? 0) + state.Temp.Where(i => i.Id == item.Id).Sum(i => i.Qty)) / Math.Max(1, item.Qty)));
    }
    bool LoadQuests(string ids) { IScriptInterface.Instance.Quests.Load(ids.Split(',').Select(int.Parse).ToArray()); return true; }
    object? SendPacket(string packet) { protocol.Call(new SendCall(packet)); Remember(packet); return null; }
    public void Remember(string packet)
    {
        if (packet.Split('%') is [_, "xt", "zm", "cmd", _, "uopref", string key, string value, ..] && bool.TryParse(value, out bool on)) preferences[key] = on;
    }
}
