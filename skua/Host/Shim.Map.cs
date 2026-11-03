using System.Text.Json.Nodes;
using Skua.Core.Models.Monsters;

using Skua.Core.Models.Players;

namespace Aqx.Skua;

public sealed partial class Shim
{
    string lastMap = "";
    string filePath = "";

    void Map()
    {
        On("IScriptMap.get_Name()", _ => MapName);
        On("IScriptMap.get_FullName()", _ =>
        {
            JsonNode map = View("map")!;
            return $"{map["name"]}-{map["room"]}";
        });
        On("IScriptMap.get_RoomID()", _ => Int(View("map")!["areaId"]));
        On("IScriptMap.get_Loaded()", _ => MapLoaded);
        On("IScriptMap.get_Loading()", _ => !MapLoaded);
        On("IScriptMap.get_PlayerNames()", _ => PlayerNames());
        On("IScriptMap.get_Players()", _ => Players());
        On("IScriptMap.get_CellPlayers()", _ => Players().FindAll(p => p.Cell == ((string?)Me["cell"] ?? "")));
        On("IScriptMap.get_PlayerCount()", _ => PlayerNames().Count);
        On("IScriptMap.get_Cells()", _ => Cells());
        On("IScriptMap.get_LastMap()", _ => lastMap);
        On("IScriptMap.set_LastMap(String)", args => lastMap = (string)args[0]!);
        On("IScriptMap.get_FilePath()", _ => filePath);
        On("IScriptMap.set_FilePath(String)", args => filePath = (string)args[0]!);
        On("IScriptMap.get_FileName()", _ => "");
        On("IScriptMap.Join(String, String, String, Boolean, Boolean)", args =>
        {
            string map = (string)args[0]!;
            lastMap = map.Split('-')[0];
            return lastMap.Equals(MapName, StringComparison.OrdinalIgnoreCase) || Join(map, (string?)args[1], (string?)args[2]);
        });
        On("IScriptMap.JoinPacket(String, String, String)", args => Join((string)args[0]!, (string?)args[1], (string?)args[2]));
        On("IScriptMap.Jump(String, String, Boolean, Boolean)", args => Jump((string)args[0]!, (string)args[1]!));
        On("IScriptMap.GetMapItem(Int32)", args => Act("send", $"%xt%zm%getMapItem%{Int(View("map")!["areaId"])}%{args[0]}%"));
        On("IScriptMap.Reload()", _ => null);
    }

    void Monsters()
    {
        On("IScriptMonster.get_MapMonsters()", _ => MapMonsters());
        On("IScriptMonster.get_CurrentMonsters()", _ => CellMonsters());
        On("IScriptMonster.get_CurrentAvailableMonsters()", _ => CellMonsters().FindAll(m => m.Alive));
        On("IScriptMonster.GetCellMonsters()", _ => MapMonsters().GroupBy(m => m.Cell).ToDictionary(g => g.Key, g => g.ToList()));
    }

    string MapName => (string?)View("map")?["name"] ?? "";

    bool MapLoaded => Phase == "ready" && MapName != "";

    List<Monster> MapMonsters() => Models.Monsters(View("map")?["monsters"]);

    Newtonsoft.Json.Linq.JArray CellMonsterData()
    {
        string cell = (string?)Me["cell"] ?? "";
        return new(View("map")?["monsters"] is JsonArray monsters ? monsters.Where(m => (string?)m?["cell"] == cell).Select(m => Models.MonsterData(m!)) : []);
    }

    List<Monster> CellMonsters()
    {
        string cell = (string?)Me["cell"] ?? "";
        return MapMonsters().FindAll(m => m.Cell == cell);
    }

    List<PlayerInfo> Players()
    {
        List<PlayerInfo> players = View("map")?["players"] is JsonArray others ? others.Select(p => Models.Player(p!)).ToList() : [];
        if (Me is { Count: > 0 } me)
            players.Insert(0, Models.Player(me));
        return players;
    }

    List<string> PlayerNames()
    {
        List<string> names = View("map")?["players"] is JsonArray players ? players.Select(p => (string?)p?["name"] ?? "").ToList() : [];
        if ((string?)Me["name"] is { Length: > 0 } me)
            names.Insert(0, me);
        return names;
    }

    List<string> Cells()
    {
        List<string> cells = ["Enter"];
        cells.AddRange(MapMonsters().Select(m => m.Cell));
        if ((string?)Me["cell"] is { Length: > 0 } mine)
            cells.Add(mine);
        return cells.Distinct().ToList();
    }

    bool Jump(string cell, string pad)
    {
        JsonObject me = Me;
        return (string?)me["cell"] == cell && (string?)me["pad"] == pad
            ? Act("send", $"%xt%zm%moveToCell%{Int(View("map")!["areaId"])}%{cell}%{pad}%")
            : Act("jump", cell, pad);
    }

    bool Join(string map, string? cell, string? pad)
    {
        return Act("join", map) && Act("jump", string.IsNullOrEmpty(cell) ? "Enter" : cell, string.IsNullOrEmpty(pad) ? "Spawn" : pad);
    }
}
