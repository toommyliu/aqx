using System.ComponentModel;

namespace Aqx.Skua;

public sealed class AqxBackground(AqxBot bot, Protocol protocol, State state) : IDisposable
{
    const int RestInterval = 1200;
    readonly CancellationTokenSource stop = new();
    readonly AutoResetEvent changed = new(false);
    Task? task;
    public void Start()
    {
        if (bot.Options.AcceptACDrops || bot.Options.AcceptAllDrops || bot.Options.RejectAllDrops) bot.Drops.Start();
        state.Updated += OnState;
        ((AqxOption)bot.Options).PropertyChanged += OnOption;
        task = Task.Factory.StartNew(Run, TaskCreationOptions.LongRunning);
    }
    void OnState() => changed.Set();
    void OnOption(object? _, PropertyChangedEventArgs e) { if (e.PropertyName == nameof(IScriptOption.RestPackets)) changed.Set(); }
    void Run()
    {
        long nextRest = 0;
        int wait = 0;
        try
        {
            while (WaitHandle.WaitAny([stop.Token.WaitHandle, changed], wait) != 0 && !bot.ShouldExit)
            {
                wait = Timeout.Infinite;
                if (!bot.Options.RestPackets || !bot.Player.Playing || bot.Player.State != 1) continue;
                long now = Environment.TickCount64;
                if (now >= nextRest)
                {
                    nextRest = now + RestInterval;
                    protocol.Call(new RestPacketCall());
                }
                wait = (int)Math.Max(0, nextRest - Environment.TickCount64);
            }
        }
        catch (OperationCanceledException) { }
    }
    public void Dispose()
    {
        bot.Drops.Stop();
        state.Updated -= OnState;
        ((AqxOption)bot.Options).PropertyChanged -= OnOption;
        stop.Cancel();
        task?.GetAwaiter().GetResult();
        stop.Dispose();
    }
}
