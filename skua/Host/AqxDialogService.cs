namespace Aqx.Skua;

public sealed class AqxDialogService(AqxBot bot) : IDialogService
{
    public bool? ShowDialog<T>(T viewModel) where T : class => null;
    public bool? ShowDialog<T>(T viewModel, string title) where T : class => null;
    public bool? ShowDialog<T>(T viewModel, Action<T> callback) where T : class => null;
    public void ShowMessageBox(string message, string caption) => bot.ShowMessageBox(message, caption);
    public bool? ShowMessageBox(string message, string caption, bool yesAndNo) => bot.ShowMessageBox(message, caption, yesAndNo);
    public DialogResult ShowMessageBox(string message, string caption, params string[] buttons) => bot.ShowMessageBox(message, caption, buttons);
}
public sealed class AqxFileDialogService : IFileDialogService
{
    public string? OpenFile() => null;
    public string? OpenFile(string filters) => null;
    public string? OpenFile(string initialDirectory, string filters) => null;
    public string? OpenFolder() => null;
    public string? OpenFolder(string initialDirectory) => null;
    public IEnumerable<string>? OpenText() => null;
    public string? Save() => null;
    public string? Save(string filters) => null;
    public string? Save(string initialDirectory, string filters) => null;
    public void SaveText(string contents) { }
    public void SaveText(IEnumerable<string> contents) { }
}
public sealed class AqxThemeService : IThemeService
{
    public event ThemeChangedEventHandler? ThemeChanged { add { } remove { } }
    public event SchemeChangedEventHandler? SchemeChanged { add { } remove { } }
    public List<object> Presets { get; } = [];
    public List<object> UserThemes { get; } = [];
    public IEnumerable<object> ColorSelectionValues => [];
    public object ColorSelectionValue { get; set; } = new();
    public IEnumerable<object> ContrastValues => [];
    public object ContrastValue { get; set; } = new();
    public float DesiredContrastRatio { get; set; }
    public bool IsColorAdjusted { get; set; }
    public bool IsDarkTheme { get; set; }
    public object? SelectedColor { get; set; }
    public ColorScheme ActiveScheme { get; set; }
    public void ApplyBaseTheme(bool isDark) => IsDarkTheme = isDark;
    public void ChangeCustomColor(object? obj) { }
    public void ChangeScheme(ColorScheme scheme) => ActiveScheme = scheme;
    public void SaveTheme(string name) { }
    public void SetCurrentTheme(object? theme) { }
    public void RemoveTheme(object? theme) { }
}
