namespace Mira.Skua;

public sealed class MiraBackground(MiraBot bot, Protocol protocol) : IDisposable
{
    readonly CancellationTokenSource stop = new();
    Task? task;
    public void Start() { if (bot.Options.AcceptACDrops || bot.Options.AcceptAllDrops || bot.Options.RejectAllDrops) bot.Drops.Start(); task = Task.Run(Run); }
    async Task Run()
    {
        long nextRest = 0;
        try
        {
            while (!stop.IsCancellationRequested && !bot.ShouldExit)
            {
                long now = Environment.TickCount64;
                if (bot.Player.Playing)
                {
                    if (bot.Options.RestPackets && bot.Player.State == 1 && now >= nextRest)
                    {
                        nextRest = now + 1200;
                        protocol.Call(new RestPacketCall());
                    }

                }
                await Task.Delay(20, stop.Token);
            }
        }
        catch (OperationCanceledException) { }
    }
    public void Dispose() { bot.Drops.Stop(); stop.Cancel(); task?.GetAwaiter().GetResult(); stop.Dispose(); }
}
