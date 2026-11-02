using System.Collections.Concurrent;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace Aqx.Skua;

public sealed class AqxException(string code, string message) : Exception(message)
{
    public string Code { get; } = code;
}

public sealed class AqxClient
{
    readonly Stream output;
    readonly object writing = new();
    readonly ConcurrentDictionary<long, TaskCompletionSource<JsonNode?>> pending = new();
    readonly BlockingCollection<(string Name, JsonNode? Data)> events = new();
    readonly CancellationTokenSource stopping = new();
    long lastId;

    public event Action<string, JsonNode?>? Event;

    public AqxClient(Stream output, TextReader input)
    {
        this.output = output;
        new Thread(() => Read(input)) { IsBackground = true, Name = "aqx reader" }.Start();
        new Thread(Dispatch) { IsBackground = true, Name = "aqx events" }.Start();
    }

    public CancellationToken Stopping => stopping.Token;

    public JsonNode? Call(string call, params object?[] args)
    {
        long id = Interlocked.Increment(ref lastId);
        TaskCompletionSource<JsonNode?> reply = new(TaskCreationOptions.RunContinuationsAsynchronously);
        pending[id] = reply;
        if (stopping.IsCancellationRequested && pending.TryRemove(id, out _))
            throw Cancelled();
        Send(new JsonObject { ["id"] = id, ["call"] = call, ["args"] = JsonSerializer.SerializeToNode(args) });
        return reply.Task.GetAwaiter().GetResult();
    }

    public void Finish(string outcome, string? detail = null)
    {
        JsonObject done = new() { ["outcome"] = outcome };
        if (detail is not null)
            done[outcome == "failed" ? "error" : "reason"] = detail;
        Send(new JsonObject { ["done"] = done });
    }

    void Send(JsonNode message)
    {
        byte[] line = Encoding.UTF8.GetBytes(message.ToJsonString() + "\n");
        lock (writing)
        {
            output.Write(line);
            output.Flush();
        }
    }

    void Read(TextReader input)
    {
        try
        {
            while (input.ReadLine() is { } line)
            {
                JsonNode? message = JsonNode.Parse(line);
                if (message?["id"] is { } id && pending.TryRemove(id.GetValue<long>(), out TaskCompletionSource<JsonNode?>? reply))
                {
                    if (message["error"] is { } error)
                        reply.SetException(new AqxException((string?)error["code"] ?? "", (string?)error["message"] ?? ""));
                    else
                        reply.SetResult(message["ok"]);
                }
                else if (message?["event"] is { } name)
                    events.Add(((string)name!, message["data"]));
                else if (message?["stop"] is not null)
                    Stop();
            }
        }
        finally
        {
            Stop();
            events.CompleteAdding();
        }
    }

    void Stop()
    {
        stopping.Cancel();
        foreach (long id in pending.Keys)
            if (pending.TryRemove(id, out TaskCompletionSource<JsonNode?>? reply))
                reply.TrySetException(Cancelled());
    }

    static AqxException Cancelled() => new("cancelled", "the script is stopping");

    void Dispatch()
    {
        foreach ((string name, JsonNode? data) in events.GetConsumingEnumerable())
        {
            try
            {
                Event?.Invoke(name, data);
            }
            catch (Exception e)
            {
                Console.Error.WriteLine(e);
            }
        }
    }
}
