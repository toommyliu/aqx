using System.Text.Json;
using Newtonsoft.Json.Linq;
using CommunityToolkit.Mvvm.Messaging;
using Skua.Core.Messaging;
namespace Mira.Skua;

public sealed class MiraEvent : IScriptEvent
{
    readonly object subscriptions = new();
    readonly Protocol protocol;
    readonly State state;
    public MiraEvent(Protocol protocol, State state)
    {
        this.protocol = protocol;
        this.state = state;
        protocol.PacketReceived += OnPacket;
        protocol.EventReceived += OnEvent;
        protocol.Capture = Capture;
    }
    ClientEvent Capture(ClientEvent data) => data switch
    {
        DropOfferedClientEvent e => e with { Item = state.Drops.FirstOrDefault(i => i.Id == e.Id)?.ToScript() },
        ItemReceivedClientEvent e when IScriptInterface.Instance is { } bot => e with { Item = e.Place switch { "bank" => bot.Bank.GetItem((int)e.Id), "temp" => bot.TempInv.GetItem((int)e.Id), _ => bot.Inventory.GetItem((int)e.Id) } },
        _ => data,
    };
    LogoutEventHandler? onLogout;
    public event LogoutEventHandler Logout
    {
        add { lock (subscriptions) { onLogout += value; Listen(); } }
        remove { lock (subscriptions) { onLogout -= value; Listen(); } }
    }
    PacketEventHandler? onPacketReceived;
    public event PacketEventHandler PacketReceived
    {
        add { lock (subscriptions) { onPacketReceived += value; Listen(); } }
        remove { lock (subscriptions) { onPacketReceived -= value; Listen(); } }
    }
    public event PlayerDeathEventHandler? PlayerDeath;
    public event MonsterKilledEventHandler? MonsterKilled;
    QuestAcceptedEventHandler? onQuestAccepted;
    public event QuestAcceptedEventHandler QuestAccepted
    {
        add { lock (subscriptions) { onQuestAccepted += value; Listen(); } }
        remove { lock (subscriptions) { onQuestAccepted -= value; Listen(); } }
    }
    public event QuestTurnInEventHandler? QuestTurnedIn;
    public event MapChangedEventHandler? MapChanged;
    CellChangedEventHandler? onCellChanged;
    public event CellChangedEventHandler CellChanged
    {
        add { lock (subscriptions) { onCellChanged += value; Listen(); } }
        remove { lock (subscriptions) { onCellChanged -= value; Listen(); } }
    }
    public event ReloginTriggeredEventHandler? ReloginTriggered;
    public void RaiseRelogin(bool kicked) => ReloginTriggered?.Invoke(kicked);
    ExtensionPacketEventHandler? onExtensionPacketReceived;
    public event ExtensionPacketEventHandler ExtensionPacketReceived
    {
        add { lock (subscriptions) { onExtensionPacketReceived += value; Listen(); } }
        remove { lock (subscriptions) { onExtensionPacketReceived -= value; Listen(); } }
    }
    public event AFKEventHandler? PlayerAFK;
    TryBuyItemHandler? onTryBuyItem;
    public event TryBuyItemHandler TryBuyItem
    {
        add { lock (subscriptions) { onTryBuyItem += value; Listen(); } }
        remove { lock (subscriptions) { onTryBuyItem -= value; Listen(); } }
    }
    public event CounterAttackHandler? CounterAttack;
    public event ItemDroppedHandler? ItemDropped;
    public event ItemSoldHandler? ItemSold;
    public event ItemBoughtHandler? ItemBought;
    public event ItemAddedToBankHandler? ItemAddedToBank;
    public event ScriptStoppingHandler? ScriptStopping;
    public event RunToAreaHandler? RunToArea;
    void Listen()
    {
        protocol.Call(new ListenCall(PacketDirection.In, onExtensionPacketReceived is not null));
        protocol.Call(new ListenCall(PacketDirection.Out, onPacketReceived is not null || onCellChanged is not null || onQuestAccepted is not null || onTryBuyItem is not null || onLogout is not null));
    }
    public void ClearHandlers()
    {
        onLogout = null;
        onPacketReceived = null;
        PlayerDeath = null;
        MonsterKilled = null;
        onQuestAccepted = null;
        QuestTurnedIn = null;
        MapChanged = null;
        onCellChanged = null;
        onExtensionPacketReceived = null;
        PlayerAFK = null;
        onTryBuyItem = null;
        CounterAttack = null;
        ItemDropped = null;
        ItemSold = null;
        ItemBought = null;
        ItemAddedToBank = null;
        ScriptStopping = null;
        RunToArea = null;
        ReloginTriggered = null;
        Listen();
    }
    public void RaiseStopping(Exception? error) => ScriptStopping?.Invoke(error);
    void OnPacket(Packet packet)
    {
        if (packet.Direction == PacketDirection.In)
        {
            JObject? extension = packet.Text switch {
                ['{', ..] => new JObject { ["params"] = new JObject { ["type"] = "json", ["dataObj"] = JObject.Parse(packet.Text).SelectToken("b.o") } },
                ['%', ..] => new JObject { ["params"] = new JObject { ["type"] = "str", ["dataObj"] = new JArray(packet.Text[1..^1].Split('%').Skip(1)) } },
                _ => null
            };
            if (extension is not null) onExtensionPacketReceived?.Invoke(extension);
            return;
        }
        string[] fields = packet.Text.Split('%');
        if (fields.Length < 5) { onPacketReceived?.Invoke(packet.Text); return; }
        switch (fields[3])
        {
            case "moveToCell" when fields.Length > 6: onCellChanged?.Invoke(state.Room?.Name.Split('-')[0] ?? "", fields[5], fields[6]); break;
            case "acceptQuest" when fields.Length > 5: onQuestAccepted?.Invoke(int.Parse(fields[5])); break;
            case "buyItem" when fields.Length > 7: onTryBuyItem?.Invoke(int.Parse(fields[6]), int.Parse(fields[5]), int.Parse(fields[7])); break;
            case "cmd" when fields.Length > 5 && fields[5] == "logout": onLogout?.Invoke(); break;
        }
        onPacketReceived?.Invoke(packet.Text);
    }
    void OnEvent(ClientEvent data)
    {
        switch (data)
        {
            case DiedClientEvent: PlayerDeath?.Invoke(); break;
            case KilledClientEvent e: MonsterKilled?.Invoke((int)e.Id); break;
            case JoinedClientEvent e: MapChanged?.Invoke(e.Map.Split('-')[0]); break;
            case QuestCompletedClientEvent e: QuestTurnedIn?.Invoke((int)e.Id); ((MiraQuest)IScriptInterface.Instance.Quests).Reaccept((int)e.Id); break;
            case AwayChangedClientEvent { Away: true }: PlayerAFK?.Invoke(); break;
            case CounterStartedClientEvent: CounterAttack?.Invoke(false); break;
            case CounterEndedClientEvent: CounterAttack?.Invoke(true); break;
            case DropOfferedClientEvent e:
                if (e.Item is { } offered)
                {
                    _ = IScriptInterface.Instance.Drops;
                    offered.Quantity = (int)e.Qty;
                    StrongReferenceMessenger.Default.Send<ItemDroppedMessage, int>(new(offered), (int)MessageChannels.GameEvents);
                    ItemDropped?.Invoke(offered, false, 0);
                }
                break;
            case ItemPickedUpClientEvent e:
                StrongReferenceMessenger.Default.Send<ItemPickedUpMessage, int>(new((int)e.Id), (int)MessageChannels.GameEvents);
                break;
            case ItemReceivedClientEvent e:
                if (e.Item is not { } received) break;
                if (e.Place == "bank")
                {
                    StrongReferenceMessenger.Default.Send<ItemAddedToBankMessage, int>(new(received, (int)e.Qty), (int)MessageChannels.GameEvents);
                    ItemAddedToBank?.Invoke(received, (int)e.Qty);
                }
                else
                {
                    StrongReferenceMessenger.Default.Send<ItemDroppedMessage, int>(new(received, true, (int)e.Qty), (int)MessageChannels.GameEvents);
                    ItemDropped?.Invoke(received, true, (int)e.Qty);
                }
                break;
            case ItemBoughtClientEvent e:
                StrongReferenceMessenger.Default.Send<ItemBoughtMessage, int>(new((int)e.CharItemId), (int)MessageChannels.GameEvents);
                ItemBought?.Invoke((int)e.CharItemId);
                break;
            case ItemSoldClientEvent e:
                StrongReferenceMessenger.Default.Send<ItemSoldMessage, int>(new((int)e.CharItemId, (int)e.Qty, (int)e.QtyNow, (int)e.Amount, e.Coins), (int)MessageChannels.GameEvents);
                ItemSold?.Invoke((int)e.CharItemId, (int)e.Qty, (int)e.QtyNow, (int)e.Amount, e.Coins);
                break;
            case ZoneClientEvent e: RunToArea?.Invoke(e.Zone ?? ""); break;
        }
    }
}
