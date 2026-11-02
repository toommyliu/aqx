using System.Reflection;
using Skua.Core.Interfaces;
using Skua.Core.Models;

namespace Aqx.Skua;

public sealed class ScriptConfig(IReadOnlyDictionary<string, string> overrides) : IScriptOptionContainer
{
    public List<IOption> Options { get; } = [];
    public Dictionary<string, List<IOption>> MultipleOptions { get; } = [];
    public Dictionary<IOption, string> OptionValues { get; } = [];
    public string Storage { get; set; } = "default";

    public string OptionsFile
    {
        get => Path.Combine(ClientFileSources.SkuaOptionsDIR, $"{Storage}.cfg");
        set { }
    }

    public void Attach(object script)
    {
        Type type = script.GetType();
        if (type.GetField("MultiOptions")?.GetValue(script) is string[] names)
            foreach (FieldInfo field in names.Select(type.GetField).OfType<FieldInfo>())
            {
                List<IOption> options = (List<IOption>)field.GetValue(script)!;
                options.ForEach(o => o.Category = field.Name.Replace('_', ' '));
                MultipleOptions.Add(field.Name, options);
            }
        if (type.GetField("Options")?.GetValue(script) is List<IOption> single)
            Options.AddRange(single);
        if (type.GetField("OptionsStorage")?.GetValue(script) is string storage)
            Storage = storage;
        SetDefaults();
        Load();
    }

    public void SetDefaults()
    {
        foreach (IOption option in Options.Concat(MultipleOptions.Values.SelectMany(o => o)))
            OptionValues[option] = option.DefaultValue?.ToString() ?? string.Empty;
    }

    public T? Get<T>(string name) where T : IConvertible => Get<T>(Options.Find(o => o.Name == name));

    public T? Get<T>(string category, string name) where T : IConvertible =>
        category == "Options" ? Get<T>(name) : Get<T>(MultipleOptions[category].Find(o => o.Name == name));

    public T? Get<T>(IOption? option) where T : IConvertible
    {
        if (option is null || !OptionValues.TryGetValue(option, out string? value) || string.IsNullOrEmpty(value))
            return default;
        return typeof(T).IsEnum ? (T)Enum.Parse(typeof(T), value.Replace(' ', '_')) : (T)Convert.ChangeType(value, typeof(T));
    }

    public string GetDirect(IOption? option) => option is null ? string.Empty : OptionValues[option];

    public void Set(string name, object value) => Set(Options.Find(o => o.Name == name), value);

    public void Set(string category, string name, object value) => Set(MultipleOptions[category].Find(o => o.Name == name), value);

    public void Set<T>(IOption? option, T value)
    {
        if (option is null)
            return;
        OptionValues[option] = value?.ToString() ?? string.Empty;
        if (!option.Transient)
            Save();
    }

    public void Configure()
    {
        SetDefaults();
        Load();
    }

    public void Load()
    {
        if (File.Exists(OptionsFile))
            foreach (string line in File.ReadLines(OptionsFile))
                Apply(line.Trim().Split([':', '='], 3));
        foreach ((string key, string value) in overrides)
            Apply([.. key.Split(':', 2), value]);
    }

    void Apply(string[] parts)
    {
        IOption? option = parts switch
        {
            ["Options", string name, _] => Options.Find(o => o.Name == name),
            [string category, string name, _] => MultipleOptions.GetValueOrDefault(category)?.Find(o => o.Name == name),
            [string name, _] => Options.Find(o => o.Name == name),
            _ => null,
        };
        if (option is not null)
            OptionValues[option] = parts[^1];
    }

    public void Save()
    {
        IEnumerable<string> lines = Options.Where(o => !o.Transient).Select(o => $"Options:{o.Name}={OptionValues[o]}")
            .Concat(MultipleOptions.SelectMany(group => group.Value.Where(o => !o.Transient).Select(o => $"{group.Key}:{o.Name}={OptionValues[o]}")));
        Directory.CreateDirectory(Path.GetDirectoryName(OptionsFile)!);
        File.WriteAllLines(OptionsFile, lines);
    }
}
