using System.Reflection;
using System.Runtime.Loader;
using System.Text.Json;
using Skua.Core.Interfaces;
using Skua.Core.Models;
using Skua.Core.Models.Skills;

namespace Aqx.Skua;

public static class Settings
{
    sealed record Row(string Group, string Name, string Label, string Description, string Kind, string[] Choices, string? Value, string? Default = null);

    public static int ScriptOptions(ScriptCompiler compiler, string path, IReadOnlyDictionary<string, string> changes)
    {
        Built built = compiler.Build(path);
        var context = new AssemblyLoadContext("script", isCollectible: true);
        Type type = Runner.ScriptMain(context.LoadFromStream(new MemoryStream(built.Image))).DeclaringType!;
        ScriptConfig config = new(new Dictionary<string, string>());
        object instance;
        try { instance = Activator.CreateInstance(type)!; }
        catch (TargetInvocationException e) { throw e.InnerException ?? e; }
        config.Attach(instance);
        List<(string Group, IOption Option)> options = [
            .. config.Options.Select(o => ("Options", o)),
            .. config.MultipleOptions.SelectMany(g => g.Value.Select(o => (g.Key, o))),
        ];
        options.RemoveAll(o => o.Option.Transient);
        foreach ((string key, string value) in changes)
        {
            (string group, string name) = key.Split(':', 2) is [var g, var n] ? (g, n) : ("Options", key);
            IOption option = options.FirstOrDefault(o => o.Group == group && o.Option.Name == name).Option
                ?? throw new ArgumentException($"the script has no option {key}");
            config.OptionValues[option] = Parse(option.Name, option.Type, value);
        }
        if (changes.Count > 0) config.Save();
        Write(config.Storage, options.Select(o =>
        {
            string kind = Kind(o.Option.Type);
            string value = config.OptionValues[o.Option];
            return new Row(o.Group, o.Option.Name, o.Option.DisplayName, o.Option.Description, kind, Choices(o.Option.Type), kind == "choice" ? value.Replace(' ', '_') : value);
        }));
        return 0;
    }

    public static int CoreBots(string user, IReadOnlyDictionary<string, string> changes)
    {
        string file = Path.Combine(ClientFileSources.SkuaOptionsDIR, $"CBO_Storage({user.ToLowerInvariant()}).txt");
        List<string> lines = File.Exists(file) ? [.. File.ReadAllLines(file)] : [];
        int Find(string key) => lines.FindIndex(l => l.Split(": ", 2)[0] == key);
        foreach ((string key, string value) in changes)
        {
            Row row = CoreBotsRows.FirstOrDefault(r => r.Name == key) ?? throw new ArgumentException($"CoreBots has no option {key}");
            int at = Find(key);
            if (value.Length == 0)
            {
                if (at >= 0) lines.RemoveAt(at);
                continue;
            }
            string line = $"{key}: {Parse(key, row.Kind switch { "bool" => typeof(bool), "number" => typeof(int), "choice" => typeof(ClassUseMode), _ => typeof(string) }, value)}";
            if (at >= 0) lines[at] = line;
            else lines.Add(line);
        }
        if (changes.Count > 0)
        {
            Directory.CreateDirectory(ClientFileSources.SkuaOptionsDIR);
            File.WriteAllLines(file, lines);
        }
        Write(null, CoreBotsRows.Select(r => r with { Value = Find(r.Name) is >= 0 and var at && lines[at].Split(": ", 2) is [_, var v] && v.Length > 0 ? v : null }));
        return 0;
    }

    static readonly string[] Modes = Enum.GetNames<ClassUseMode>();

    static readonly Row[] CoreBotsRows = [
        Bool("Options", "PrivateRooms", "Private rooms", true),
        Bool("Options", "PublicDifficult", "Public on difficult parts", false),
        Bool("Options", "BankMiscAC", "Bank misc. AC items on start-up", false),
        Bool("Options", "LoggerInChat", "Logger in chat", true),
        Bool("Options", "MessageBoxCheck", "Force off message boxes", false),
        Bool("Options", "RestCheck", "Should rest", false),
        Bool("Options", "DisableAutoEnhance", "Disable auto enhance", false),
        Bool("Options", "DisableBestGear", "Disable best gear", false),
        Bool("Options", "AntiLag", "Anti lag", true),
        Bool("Options", "IncognitoMode", "Incognito mode", false),
        Number("Options", "PrivateRoomNr", "Room number", 100000),
        Number("Options", "ActionDelayNr", "Action delay", 700),
        Number("Options", "ExitCombatNr", "Exit combat delay", 1600),
        Number("Options", "HuntDelayNr", "Hunt delay", 1000),
        Number("Options", "QuestTriesNr", "Accept and complete tries", 20),
        Number("Options", "QuestMaxNr", "Loaded quests max.", 150),
        Text("Options", "StopLocationSelect", "Map after stopping the bot", "whitemap"),
        .. new[] { "Solo", "Farm", "Dodge", "Boss" }.SelectMany(Loadout),
        Bool("Boosters", "doGoldBoost", "Use gold boosts when farming gold", false),
        Bool("Boosters", "doClassBoost", "Use class boosts when ranking up a class", false),
        Bool("Boosters", "doRepBoost", "Use reputation boosts when farming rep", false),
        Bool("Boosters", "doExpBoost", "Use experience boosts when farming exp", false),
        Bool("Nation farms", "Nation_SellMemVoucher", "Sell Voucher of Nulgath if not needed", false),
        Bool("Nation farms", "Nation_ReturnPolicyDuringSupplies", "Do Swindles Return during supplies", false),
        Bool("Nation farms", "UltraAlteonForSupplies", "Ultra Alteon for supplies", false),
        Bool("Bludrut Brawl", "PvP_SoloPvPBoss", "Kill ads before boss", false),
        Bool("Bot creators", "BCO_Story_TestBot", "Story bot test mode", false),
    ];

    static IEnumerable<Row> Loadout(string role, int i) => [
        Text("Loadout", $"{role}ClassSelect", $"{role} class"),
        Bool("Loadout", $"{role}EquipCheck", $"{role} equipment", true),
        new Row("Loadout", $"{role}ModeSelect", $"{role} class mode", "", "choice", Modes, null, nameof(ClassUseMode.Base)),
        .. new[] { "Helm", "Armor", "Cape", "Weapon", "Pet", "GroundItem" }.Select(slot =>
            Text("Loadout", $"{slot}{i + 1}Select", $"{role} {(slot == "GroundItem" ? "ground item" : slot.ToLowerInvariant())}")),
    ];

    static Row Bool(string group, string key, string label, bool fallback) => new(group, key, label, "", "bool", [], null, fallback.ToString());
    static Row Number(string group, string key, string label, int fallback) => new(group, key, label, "", "number", [], null, fallback.ToString());
    static Row Text(string group, string key, string label, string? fallback = null) => new(group, key, label, "", "text", [], null, fallback);

    static string Kind(Type type) =>
        type == typeof(bool) ? "bool"
        : type.IsEnum ? "choice"
        : type.IsPrimitive && type != typeof(char) || type == typeof(decimal) ? "number"
        : "text";

    static string[] Choices(Type type) => type.IsEnum ? Enum.GetNames(type) : [];

    static string Parse(string name, Type type, string value)
    {
        if (value.Contains('\n') || value.Contains('\r')) throw new ArgumentException($"{name} must fit on one line");
        if (type == typeof(bool)) return bool.TryParse(value, out bool b) ? b.ToString() : throw new ArgumentException($"{name} must be True or False");
        if (type.IsEnum)
        {
            string[] names = Enum.GetNames(type);
            return names.Contains(value.Replace(' ', '_')) ? value.Replace(' ', '_') : throw new ArgumentException($"{name} must be one of {string.Join(", ", names)}");
        }
        try { Convert.ChangeType(value, type); }
        catch (Exception e) when (e is FormatException or OverflowException) { throw new ArgumentException($"{name} must be a {(Kind(type) == "number" ? "number" : type.Name)}"); }
        return value;
    }

    static void Write(string? storage, IEnumerable<Row> rows)
    {
        using Utf8JsonWriter json = new(Console.OpenStandardOutput());
        json.WriteStartObject();
        if (storage is not null) json.WriteString("storage", storage);
        json.WriteStartArray("options");
        foreach (Row row in rows)
        {
            json.WriteStartObject();
            json.WriteString("group", row.Group);
            json.WriteString("name", row.Name);
            json.WriteString("label", row.Label);
            json.WriteString("description", row.Description);
            json.WriteString("kind", row.Kind);
            json.WriteStartArray("choices");
            foreach (string choice in row.Choices) json.WriteStringValue(choice);
            json.WriteEndArray();
            if (row.Value is null) json.WriteNull("value");
            else json.WriteString("value", row.Value);
            if (row.Default is null) json.WriteNull("default");
            else json.WriteString("default", row.Default);
            json.WriteEndObject();
        }
        json.WriteEndArray();
        json.WriteEndObject();
    }
}
