using System.Text.Json;
using System.Text.Json.Nodes;
namespace Mira.Skua;

public static class ProtocolFixtures
{
    public static int Check(string path)
    {
        int count = 0;
        foreach (string file in new[] { "requests", "worker", "messages" })
        {
            using var document = JsonDocument.Parse(File.ReadAllText(Path.Combine(path, file + ".json")));
            foreach (JsonElement fixture in document.RootElement.EnumerateArray())
            {
                var type = file == "requests" ? Protocol.Json.Request : file == "worker" ? (System.Text.Json.Serialization.Metadata.JsonTypeInfo)Protocol.Json.Completion :
                    fixture.TryGetProperty("reply", out _) ? fixture.TryGetProperty("ok", out _) ? Protocol.Json.SuccessReply : Protocol.Json.FailureReply :
                    fixture.TryGetProperty("state", out _) ? Protocol.Json.StateMessage :
                    fixture.TryGetProperty("event", out _) ? Protocol.Json.EventMessage :
                    fixture.TryGetProperty("packet", out _) ? Protocol.Json.PacketMessage : fixture.TryGetProperty("disconnected", out _) ? Protocol.Json.DisconnectedMessage : fixture.TryGetProperty("reset", out _) ? Protocol.Json.ResetMessage : Protocol.Json.StopMessage;
                object value = JsonSerializer.Deserialize(fixture.GetRawText(), type)!;
                string encoded = JsonSerializer.Serialize(value, type);
                if (!JsonNode.DeepEquals(JsonNode.Parse(fixture.GetRawText()), JsonNode.Parse(encoded))) throw new InvalidOperationException($"protocol fixture changed: {fixture} => {encoded}");
                if (value is StateMessage message) new State().Install(message.State);
                count++;
            }
        }
        Console.WriteLine($"{count} protocol fixtures passed");
        return 0;
    }
}
