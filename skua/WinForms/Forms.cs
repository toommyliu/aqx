using System.Drawing;

namespace System.Windows.Forms;

public delegate void MethodInvoker();

public delegate void PaintEventHandler(object? sender, PaintEventArgs e);

public enum DockStyle { None, Top, Bottom, Left, Right, Fill }

public enum ProgressBarStyle { Blocks, Continuous, Marquee }

public enum FormStartPosition { Manual, CenterScreen, WindowsDefaultLocation, WindowsDefaultBounds, CenterParent }

public enum FormBorderStyle { None, FixedSingle, Fixed3D, FixedDialog, Sizable, FixedToolWindow, SizableToolWindow }

public class PaintEventArgs : EventArgs;

public class Control : IDisposable
{
    public string Text { get; set; } = "";
    public Size Size { get; set; }
    public int Width => Size.Width;
    public int Height => Size.Height;
    public DockStyle Dock { get; set; }
    public Color ForeColor { get; set; }
    public Color BackColor { get; set; }
    public ControlCollection Controls { get; } = new();

    public event PaintEventHandler? Paint;

    public object? Invoke(Delegate method) => method.DynamicInvoke();

    public void Dispose() { }

    public class ControlCollection
    {
        public void Add(Control control) { }
    }
}

public class Form : Control
{
    public FormStartPosition StartPosition { get; set; }
    public FormBorderStyle FormBorderStyle { get; set; }
    public bool MaximizeBox { get; set; }
    public bool MinimizeBox { get; set; }

    public event EventHandler? Shown;

    public void Close() { }
}

public class ProgressBar : Control
{
    public int Minimum { get; set; }
    public int Maximum { get; set; }
    public int Value { get; set; }
    public ProgressBarStyle Style { get; set; }
}

public static class Application
{
    public static void Run(Form form) { }
}

public enum DialogResult { None, OK, Cancel, Abort, Retry, Ignore, Yes, No }

public enum MessageBoxButtons { OK, OKCancel, AbortRetryIgnore, YesNoCancel, YesNo, RetryCancel }

public enum MessageBoxIcon { None, Error, Question, Warning, Information }

public static class MessageBox
{
    public static DialogResult Show(string text, string caption = "", MessageBoxButtons buttons = MessageBoxButtons.OK, MessageBoxIcon icon = MessageBoxIcon.None) =>
        DialogResult.Cancel;
}
