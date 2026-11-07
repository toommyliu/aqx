namespace Mira.Skua;

public sealed class MiraMonster(State state) : IScriptMonster
{
    public List<Monster> CurrentMonsters => state.Room?.Monsters.Where(m => m.Cell == state.Room.Cell).Select(m => m.ToScript()).ToList() ?? [];
    public List<Monster> MapMonsters => state.Room?.Monsters.Select(m => m.ToScript()).ToList() ?? [];
    public List<Monster> CurrentAvailableMonsters => CurrentMonsters;
    public Dictionary<string, List<Monster>> GetCellMonsters() => IScriptInterface.Instance.Map.Cells.ToDictionary(cell => cell, cell => MapMonsters.Where(m => m.Cell == cell).ToList());
}
