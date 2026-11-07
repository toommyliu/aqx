namespace Mira.Skua;

public sealed class MiraHandlers(MiraBot bot) : IScriptHandlers
{
    sealed class TimedHandler(string name, int ticks, Func<IScriptInterface, bool> function) : IHandler
    {
        public string Name => name;
        public int Ticks => ticks;
        public Func<IScriptInterface, bool> Function => function;
        public long Next;
    }
    readonly List<TimedHandler> handlers = [];
    Thread? timer;
    int next;
    public IEnumerable<IHandler> CurrentHandlers { get { lock (handlers) return handlers.ToArray(); } }
    public IHandler RegisterHandler(int ticks, Func<IScriptInterface, bool> function, string name = null!)
    {
        lock (handlers)
        {
            TimedHandler handler = new(name ?? (next++).ToString(), ticks, function);
            handlers.Add(handler);
            if (timer is null)
            {
                timer = new(Run) { IsBackground = true, Name = "Script handlers" };
                timer.Start();
            }
            return handler;
        }
    }
    public bool Remove(string name) { lock (handlers) return handlers.RemoveAll(h => h.Name == name) > 0; }
    public bool Remove(IHandler handler) { lock (handlers) return handlers.RemoveAll(h => ReferenceEquals(h, handler)) > 0; }
    public bool Remove(List<IHandler> removing) { lock (handlers) return handlers.RemoveAll(removing.Contains) == removing.Count; }
    public void Clear() { lock (handlers) handlers.Clear(); }
    public bool Stop()
    {
        Clear();
        if (timer is not null && !timer.Join(TimeSpan.FromSeconds(2))) return false;
        timer = null;
        return true;
    }
    void Run()
    {
        var token = bot.ScriptCts.Token;
        while (!token.WaitHandle.WaitOne(20))
        {
            TimedHandler[] due;
            lock (handlers)
            {
                long now = Environment.TickCount64;
                due = handlers.Where(h => now >= h.Next).ToArray();
                foreach (var handler in due) handler.Next = now + (long)handler.Ticks * 20;
            }
            foreach (var handler in due)
            {
                if (token.IsCancellationRequested) return;
                try { if (!handler.Function(bot)) Remove(handler); }
                catch (OperationCanceledException) when (bot.ShouldExit) { return; }
                catch (Exception e) { bot.Log($"handler {handler.Name} threw: {e.Message}"); }
            }
        }
    }
}
