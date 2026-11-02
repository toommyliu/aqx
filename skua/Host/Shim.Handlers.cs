using Skua.Core.Interfaces;

namespace Aqx.Skua;

public sealed partial class Shim
{
    const int TimerDelay = 20;

    readonly List<TimedHandler> timed = [];
    readonly Dictionary<string, long> lastRun = [];
    Thread? timer;
    int nextHandler;

    sealed record TimedHandler(string Name, int Ticks, Func<IScriptInterface, bool> Function) : IHandler;

    void Handlers()
    {
        On("IScriptHandlers.get_CurrentHandlers()", _ => Snapshot());
        On("IScriptHandlers.RegisterHandler(Int32, Func`2, String)", args =>
        {
            TimedHandler handler = new((string?)args[2] ?? Interlocked.Increment(ref nextHandler).ToString(), (int)args[0]!, (Func<IScriptInterface, bool>)args[1]!);
            lock (timed)
            {
                timed.Add(handler);
                timer ??= StartTimer();
            }
            return handler;
        });
        On("IScriptHandlers.Remove(String)", args => Remove(h => h.Name == (string?)args[0]));
        On("IScriptHandlers.Remove(IHandler)", args => Remove(h => ReferenceEquals(h, args[0])));
        On("IScriptHandlers.Remove(List`1)", args =>
        {
            List<IHandler> removing = (List<IHandler>)args[0]!;
            return Remove(removing.Contains) == removing.Count;
        });
        On("IScriptHandlers.Clear()", _ => Remove(_ => true));
    }

    List<IHandler> Snapshot()
    {
        lock (timed)
            return [.. timed];
    }

    int Remove(Predicate<IHandler> which)
    {
        lock (timed)
            return timed.RemoveAll(h => which(h));
    }

    Thread StartTimer()
    {
        Thread thread = new(RunHandlers) { IsBackground = true, Name = "Skua handlers" };
        thread.Start();
        return thread;
    }

    void RunHandlers()
    {
        while (!Aqx.Stopping.IsCancellationRequested)
        {
            Thread.Sleep(TimerDelay);
            List<IHandler> due = Snapshot().Where(h => !lastRun.TryGetValue(h.Name, out long at) || Environment.TickCount64 - at >= h.Ticks * TimerDelay).ToList();
            if (due.Count == 0 || !Playing)
                continue;
            foreach (TimedHandler handler in due.Cast<TimedHandler>())
            {
                lastRun[handler.Name] = Environment.TickCount64;
                bool keep;
                try
                {
                    keep = handler.Function(IScriptInterface.Instance);
                }
                catch (Exception e)
                {
                    Aqx.Call("log", "warn", $"handler {handler.Name} threw: {e.Message}");
                    keep = true;
                }
                if (!keep)
                    Remove(h => ReferenceEquals(h, handler));
            }
        }
    }
}
