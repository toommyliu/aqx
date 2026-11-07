namespace Mira.Skua;

public sealed class MiraDialogService(MiraBot bot) : IDialogService
{
    public bool? ShowDialog<T>(T viewModel) where T : class => null;
    public bool? ShowDialog<T>(T viewModel, string title) where T : class => null;
    public bool? ShowDialog<T>(T viewModel, Action<T> callback) where T : class => null;
    public void ShowMessageBox(string message, string caption) => bot.ShowMessageBox(message, caption);
    public bool? ShowMessageBox(string message, string caption, bool yesAndNo) => bot.ShowMessageBox(message, caption, yesAndNo);
    public DialogResult ShowMessageBox(string message, string caption, params string[] buttons) => bot.ShowMessageBox(message, caption, buttons);
}
