using System.Collections.Concurrent;
using System.Reflection;
using System.Text.Json.Nodes;
using Skua.Core.Interfaces;
using Skua.Core.Models.Items;

namespace Aqx.Skua;

public sealed partial class Shim
{
    static readonly TimeSpan StoppingTimeout = TimeSpan.FromSeconds(2);

    readonly ConcurrentDictionary<string, Delegate?> events = new();
    volatile bool runStopping = true;

    void Events()
    {
        foreach (EventInfo e in typeof(IScriptEvent).GetEvents())
        {
            string name = e.Name;
            On(Key(typeof(IScriptEvent), e.AddMethod!), args => events.AddOrUpdate(name, (Delegate)args[0]!, (_, d) => Delegate.Combine(d, (Delegate)args[0]!)));
            On(Key(typeof(IScriptEvent), e.RemoveMethod!), args => events.AddOrUpdate(name, _ => null, (_, d) => Delegate.Remove(d, (Delegate)args[0]!)));
        }
        Aqx.Event += Forward;
    }

    void Forward(string name, JsonNode? data)
    {
        switch (name)
        {
            case "death":
                Raise("PlayerDeath");
                break;
            case "kill":
                Raise("MonsterKilled", Int(data?["mapId"]));
                break;
            case "quest":
                Raise((string?)data?["change"] == "completed" ? "QuestTurnedIn" : "QuestAccepted", Int(data?["id"]));
                break;
            case "map":
                Raise("MapChanged", (string?)data?["name"] ?? "");
                break;
            case "cell":
                Raise("CellChanged", MapName, (string?)data?["cell"] ?? "", (string?)data?["pad"] ?? "");
                break;
            case "drop" when data is not null:
                Raise("ItemDropped", Models.Item<ItemBase>(data), false, Int(data["qty"]));
                break;
            case "counter":
                bool active = (bool?)data?["active"] ?? false;
                Countering(active);
                Raise("CounterAttack", !active);
                break;
        }
    }

    void Raise(string name, params object?[] args)
    {
        if (events.GetValueOrDefault(name) is not { } handlers)
            return;
        foreach (Delegate handler in handlers.GetInvocationList())
        {
            try
            {
                handler.DynamicInvoke(args);
            }
            catch (TargetInvocationException e)
            {
                Aqx.Call("log", "warn", $"a {name} handler threw: {e.InnerException?.Message}");
            }
        }
    }

    public void ScriptStopping(Exception? exception)
    {
        if (!runStopping || events.GetValueOrDefault(nameof(IScriptEvent.ScriptStopping)) is not { } handlers)
            return;
        Task raised = Task.Run(() =>
        {
            foreach (ScriptStoppingHandler handler in handlers.GetInvocationList().Cast<ScriptStoppingHandler>())
            {
                try
                {
                    handler(exception);
                }
                catch (Exception) when (Aqx.Stopping.IsCancellationRequested)
                {
                    return;
                }
                catch (Exception e)
                {
                    Aqx.Call("log", "warn", $"a ScriptStopping handler threw: {e.Message}");
                }
            }
        });
        raised.Wait(StoppingTimeout);
    }
}
