using System.Text.Json.Nodes;
using Skua.Core.Models.Items;
using Skua.Core.Interfaces;

namespace Aqx.Skua;

public sealed partial class Shim
{
    const int GrabberPeriod = 1000;

    readonly List<string> wantedNames = [];
    readonly List<int> wantedIds = [];
    CancellationTokenSource? grabbing;
    int dropInterval;
    bool rejectElse;

    void Drops()
    {
        On("IScriptDrop.get_CurrentDropInfos()", _ => CurrentDrops());
        On("IScriptDrop.get_CurrentDrops()", _ => CurrentDrops().Select(d => d.Name.Trim()).ToList());
        On("IScriptDrop.get_Enabled()", _ => grabbing is { IsCancellationRequested: false });
        On("IScriptDrop.get_Interval()", _ => dropInterval);
        On("IScriptDrop.set_Interval(Int32)", args => dropInterval = (int)args[0]!);
        On("IScriptDrop.get_RejectElse()", _ => rejectElse);
        On("IScriptDrop.set_RejectElse(Boolean)", args => rejectElse = (bool)args[0]!);
        On("IScriptDrop.get_ToPickup()", _ => Locked(wantedNames, () => wantedNames.ToList()));
        On("IScriptDrop.get_ToPickupIDs()", _ => Locked(wantedIds, () => wantedIds.ToList()));
        On("IScriptDrop.Add(String[])", args => Locked(wantedNames, () => wantedNames.AddRange(((string[])args[0]!).Where(n => !wantedNames.Contains(n, StringComparer.OrdinalIgnoreCase)))));
        On("IScriptDrop.Add(Int32[])", args => Locked(wantedIds, () => wantedIds.AddRange(((int[])args[0]!).Except(wantedIds))));
        On("IScriptDrop.Remove(String[])", args => Locked(wantedNames, () => wantedNames.RemoveAll(n => ((string[])args[0]!).Contains(n, StringComparer.OrdinalIgnoreCase))));
        On("IScriptDrop.Remove(Int32[])", args => Locked(wantedIds, () => wantedIds.RemoveAll(((int[])args[0]!).Contains)));
        On("IScriptDrop.Clear()", _ =>
        {
            Locked(wantedNames, wantedNames.Clear);
            Locked(wantedIds, wantedIds.Clear);
            return null;
        });
        On("IScriptDrop.Start()", _ => StartGrabber());
        On("IScriptDrop.Stop()", _ => StopGrabber());
        On("IScriptDrop.StopAsync()", _ =>
        {
            StopGrabber();
            return ValueTask.CompletedTask;
        });
        On("IScriptDrop.Pickup(String)", args => PickupDrop((string)args[0]!));
        On("IScriptDrop.Pickup(Int32)", args => PickupDrop((int)args[0]!));
        On("IScriptDrop.Pickup(String[])", args => PickupEach((string[])args[0]!, name => PickupDrop(name), name => IScriptInterface.Instance.Wait.ForPickup(name)));
        On("IScriptDrop.Pickup(Int32[])", args => PickupEach((int[])args[0]!, id => PickupDrop(id), id => IScriptInterface.Instance.Wait.ForPickup(id)));
        On("IScriptDrop.PickupAll(Boolean)", args =>
        {
            CurrentDrops().ForEach(d => PickupDrop(d.Name));
            return (bool)args[0]! || IScriptInterface.Instance.Wait.ForPickup("*");
        });
        On("IScriptDrop.RejectAll(Boolean)", _ => null);
        On("IScriptDrop.RejectExcept(String[])", _ => null);
        On("IScriptDrop.RejectExcept(Int32[])", _ => null);
        On("IScriptDrop.add_PropertyChanged(PropertyChangedEventHandler)", _ => null);
        On("IScriptDrop.remove_PropertyChanged(PropertyChangedEventHandler)", _ => null);
    }

    static T Locked<T>(object gate, Func<T> read)
    {
        lock (gate)
            return read();
    }

    static object? Locked(object gate, Action change)
    {
        lock (gate)
            change();
        return null;
    }

    List<ItemBase> CurrentDrops() => Models.Items<ItemBase>(View("drops"));

    object? PickupDrop(string name) => Aqx.Call("request_pickup", name);

    object? PickupDrop(int id) => CurrentDrops().Find(d => d.ID == id) is { } drop ? PickupDrop(drop.Name) : null;

    static object? PickupEach<T>(T[] drops, Action<T> pickup, Func<T, bool> wait)
    {
        foreach (T drop in drops)
        {
            pickup(drop);
            wait(drop);
        }
        return null;
    }

    object? StartGrabber()
    {
        if (grabbing is { IsCancellationRequested: false })
            return null;
        CancellationTokenSource running = grabbing = new();
        Background("Skua drops", () => Grab(running.Token));
        return null;
    }

    object? StopGrabber()
    {
        grabbing?.Cancel();
        return null;
    }

    void Grab(CancellationToken stop)
    {
        while (!stop.IsCancellationRequested && !Aqx.Stopping.IsCancellationRequested)
        {
            Thread.Sleep(GrabberPeriod + dropInterval);
            if (!Playing || View("drops") is not JsonArray { Count: > 0 })
                continue;
            if (Option("AcceptAllDrops"))
            {
                CurrentDrops().ForEach(d => PickupDrop(d.Name));
                continue;
            }
            if (Option("AcceptACDrops"))
                CurrentDrops().Where(d => d.Coins).ToList().ForEach(d => PickupDrop(d.Name));
            if (Option("RejectAllDrops"))
                continue;
            Locked(wantedIds, () => wantedIds.ToList()).ForEach(id => PickupDrop(id));
            Locked(wantedNames, () => wantedNames.ToList()).ForEach(name => PickupDrop(name));
        }
    }

    bool Option(string name) => settings.GetValueOrDefault($"IScriptOption.{name}") is true;
}
