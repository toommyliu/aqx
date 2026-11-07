using System.Collections.Concurrent;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Mira.Skua;

public sealed class Protocol : IDisposable
{
    internal static readonly ProtocolJson Json = new(new JsonSerializerOptions { AllowOutOfOrderMetadataProperties = true, RespectRequiredConstructorParameters = true, PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower, Converters = { new JsonStringEnumConverter(JsonNamingPolicy.SnakeCaseLower, allowIntegerValues: false) } });
    readonly TextReader input;
    readonly StreamWriter output;
    readonly ConcurrentDictionary<long, TaskCompletionSource<ReplyMessage>> pending = new();
    readonly BlockingCollection<Action> events = new();
    readonly CancellationTokenSource stopped = new();
    readonly CancellationTokenSource closed = new();
    readonly Thread reader;
    readonly Thread dispatcher;
    long next;
    long generation;
    public CancellationToken Stopping => stopped.Token;
    public event Action? StopRequested;
    public event Action<bool>? Disconnected;
    public event Action<Packet>? PacketReceived;
    public event Action<ClientEvent>? EventReceived;
    public event Action<SectionUpdate>? StateReceived;

    public Protocol(Stream output, TextReader input, State state)
    {
        this.input = input;
        this.output = new(output) { AutoFlush = true };
        reader = new(() => Read(state)) { IsBackground = true, Name = "Protocol reader" };
        dispatcher = new(() => {
            foreach (Action action in events.GetConsumingEnumerable())
                try { action(); } catch (Exception e) { Console.Error.WriteLine(e.Message); }
        }) { IsBackground = true, Name = "Script events" };
        dispatcher.Start();
        reader.Start();
    }

    public JsonElement? Call(Request request, CancellationToken token = default)
    {
        long id = Interlocked.Increment(ref next);
        TaskCompletionSource<ReplyMessage> completion = new(TaskCreationOptions.RunContinuationsAsynchronously);
        pending[id] = completion;
        try
        {
            Write(JsonSerializer.Serialize(request with { Id = id }, Json.Request));
            using var waiting = CancellationTokenSource.CreateLinkedTokenSource(closed.Token, token);
            using var cancellation = token.Register(() => { if (request is ReloginCall) Write(JsonSerializer.Serialize<Request>(new CancelReloginCall(id) { Id = Interlocked.Increment(ref next) }, Json.Request)); });
            ReplyMessage reply = completion.Task.WaitAsync(waiting.Token).GetAwaiter().GetResult();
            if (reply.Error is { } error)
            {
                if (error.Kind == ErrorKind.Cancelled || error.Kind == ErrorKind.Closed) throw new OperationCanceledException(error.Message);
                throw new InvalidOperationException($"{error.Kind}: {error.Message}");
            }
            return reply.Ok;
        }
        finally { pending.TryRemove(id, out _); }
    }

    void Read(State state)
    {
        try
        {
            while (input.ReadLine() is { } line)
            {
                using JsonDocument document = JsonDocument.Parse(line);
                JsonElement root = document.RootElement;
                if (root.TryGetProperty("reply", out _))
                {
                    ReplyMessage reply;
                    if (root.TryGetProperty("error", out _)) { var failure = JsonSerializer.Deserialize(line, Json.FailureReply)!; reply = new(failure.Reply, null, failure.Error); }
                    else { var success = JsonSerializer.Deserialize(line, Json.SuccessReply)!; reply = new(success.Reply, success.Ok, null); }
                    if (pending.TryGetValue(reply.Reply, out var completion)) completion.TrySetResult(reply);
                }
                else if (root.TryGetProperty("state", out _))
                {
                    var section = JsonSerializer.Deserialize(line, Json.StateMessage)!.State;
                    state.Install(section);
                    long current = Volatile.Read(ref generation);
                    events.Add(() => { if (current == Volatile.Read(ref generation)) StateReceived?.Invoke(section); });
                }
                else if (root.TryGetProperty("event", out _))
                {
                    var message = JsonSerializer.Deserialize(line, Json.EventMessage)!;
                    long current = Volatile.Read(ref generation);
                    events.Add(() => { if (current == Volatile.Read(ref generation)) EventReceived?.Invoke(message.Event); });
                }
                else if (root.TryGetProperty("packet", out _))
                {
                    var message = JsonSerializer.Deserialize(line, Json.PacketMessage)!;
                    long current = Volatile.Read(ref generation);
                    events.Add(() => { if (current == Volatile.Read(ref generation)) PacketReceived?.Invoke(message.Packet); });
                }
                else if (root.TryGetProperty("disconnected", out var disconnected)) Disconnected?.Invoke(disconnected.GetBoolean());
                else if (root.TryGetProperty("stop", out _)) Stop();
                else if (root.TryGetProperty("reset", out _)) { Interlocked.Increment(ref generation); state.Reset(); }
                else throw new JsonException("unknown worker message");
            }
        }
        catch (Exception e) { Console.Error.WriteLine(e.Message); }
        finally
        {
            Stop();
            closed.Cancel();
            foreach (var completion in pending.Values) completion.TrySetCanceled();
        }
    }
    void Stop() { if (!stopped.IsCancellationRequested) { stopped.Cancel(); StopRequested?.Invoke(); } }
    void Write(string line) { lock (output) output.WriteLine(line); }
    public void Finish(EndOutcome outcome, string? detail) => Write(JsonSerializer.Serialize(new Completion(new(outcome, detail)), Json.Completion));
    public void Dispose() { events.CompleteAdding(); output.Dispose(); }
}

[AttributeUsage(AttributeTargets.Property | AttributeTargets.Method | AttributeTargets.Event)]
public sealed class UnsupportedAttribute : Attribute;
