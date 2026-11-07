using CommunityToolkit.Mvvm.Messaging;

namespace Mira.Skua;

public sealed class MiraQuest(Protocol protocol, State state) : IScriptQuest
{
    readonly Dictionary<int, int> registered = [];
    readonly Dictionary<int, long> lastAccept = [];
    readonly Dictionary<int, long> lastComplete = [];
    CancellationTokenSource? pollingStop;
    Task? poller;
    MiraBot Bot => (MiraBot)IScriptInterface.Instance;
    public int RegisterCompleteInterval { get; set; } = 2000;
    public List<Quest> Tree => state.Quests.ScriptTree;
    public List<Quest> Active => Tree.FindAll(q => q.Active);
    public List<Quest> Completed => Tree.FindAll(q => q.Status == "c");
    public Dictionary<int, QuestData> CachedDictionary { get; set; } = [];
    bool cacheLoaded;
    public List<QuestData> Cached { get => CachedDictionary.Values.ToList(); set { CachedDictionary.Clear(); foreach (var quest in value) CachedDictionary[quest.ID] = quest; } }
    public IEnumerable<int> Registered { get { lock (registered) return registered.Keys.ToList(); } }
    public IReadOnlyDictionary<int, int> RegisteredRewards { get { lock (registered) return new Dictionary<int, int>(registered); } }
    public event PropertyChangedEventHandler? PropertyChanged;
    public void Load(params int[] ids)
    {
        foreach (int[] chunk in ids.Chunk(30))
        {
            Bot.ThrowIfStopped();
            protocol.Call(new LoadQuestsCall(chunk.Select(id => (long)id).ToArray()));
            if (ids.Length >= 30) Bot.Sleep(Bot.Options.ActionDelay);
        }
    }
    public Quest? EnsureLoad(int id)
    {
        Bot.Wait.ForTrue(() => TryGetQuest(id, out _), () => Load(id), 20, 1000);
        TryGetQuest(id, out var quest);
        return quest;
    }
    public bool TryGetQuest(int id, out Quest? quest) => (quest = Tree.Find(q => q.ID == id)) is not null;
    public bool IsInProgress(int id) => state.Quests.Accepted.Contains(id);
    public bool CanComplete(int id) => state.Quests.Status.GetValueOrDefault(id) == "completable";
    public bool CanCompleteFullCheck(int id)
    {
        if (CanComplete(id)) return true;
        if (EnsureLoad(id) is not { } quest) return false;
        return quest.Requirements.Concat(quest.AcceptRequirements).All(i => Bot.InvHelper.Check(i.ID, i.Quantity, false));
    }
    public bool IsDailyComplete(int id) => EnsureLoad(id) is { } quest && IsDailyComplete(quest);
    public bool IsDailyComplete(Quest quest) => Bot.Flash.CallGameFunction<int>("world.getAchievement", quest.Field, quest.Index) > 0;
    public bool IsUnlocked(int id) => EnsureLoad(id) is { } quest && IsUnlocked(quest);
    public bool IsUnlocked(Quest quest) => quest.Slot < 0 || Bot.Flash.CallGameFunction<int>("world.getQuestValue", quest.Slot) >= quest.Value - 1;
    public bool HasBeenCompleted(int id) => EnsureLoad(id) is { } quest && HasBeenCompleted(quest);
    public bool HasBeenCompleted(Quest quest) => quest.Slot < 0 || Bot.Flash.CallGameFunction<int>("world.getQuestValue", quest.Slot) >= quest.Value;
    public bool IsAvailable(int id) => EnsureLoad(id) is { } q && !IsDailyComplete(q) && IsUnlocked(q)
        && (!q.Upgrade || Bot.Player.IsMember) && Bot.Player.Level >= q.Level
        && (q.RequiredClassID <= 0 || (state.Inventory.ById.GetValueOrDefault(q.RequiredClassID)?.Qty ?? -1) >= q.RequiredClassPoints)
        && (q.RequiredFactionId <= 1 || (state.Factions.FirstOrDefault(f => f.Id == q.RequiredFactionId)?.Rep ?? 0) >= q.RequiredFactionRep)
        && q.AcceptRequirements.All(r => Bot.Inventory.Contains(r.Name, r.Quantity));
    bool Act(Request request)
    {
        Bot.ThrowIfStopped();
        try { protocol.Call(request); return true; }
        catch (InvalidOperationException) { return false; }
    }
    public bool Accept(int id)
    {
        Bot.ThrowIfStopped();
        try { protocol.Call(new AcceptCall(id)); }
        catch (CallFailedException e) when (e.Kind is ErrorKind.Refused or ErrorKind.NoReply) { }
        catch (InvalidOperationException) { return IsInProgress(id); }
        return true;
    }
    public void Accept(params int[] ids) { foreach (int id in ids) { Accept(id); Bot.Sleep(Bot.Options.ActionDelay); } }
    public bool EnsureAccept(int id)
    {
        for (int i = 0; i < Bot.Options.QuestAcceptAndCompleteTries; i++)
        {
            if (Accept(id)) return true;
            Bot.Sleep(Bot.Options.ActionDelay);
        }
        return IsInProgress(id);
    }
    public void EnsureAccept(params int[] ids) { foreach (int id in ids) { EnsureAccept(id); Bot.Sleep(Bot.Options.ActionDelay); } }
    public bool Complete(int id, int itemId = -1, bool special = false) { Act(new CompleteCall(id, itemId < 0 ? null : itemId, 1, special)); return !IsInProgress(id); }
    public void Complete(params int[] ids) { foreach (int id in ids) { Complete(id); Bot.Sleep(Bot.Options.ActionDelay); } }
    public bool EnsureComplete(int id, int itemId = -1, bool special = false)
    {
        if (id == 0) return !IsInProgress(id);
        for (int i = 0; i < Bot.Options.QuestAcceptAndCompleteTries; i++)
        {
            Complete(id, itemId, special);
            if (!IsInProgress(id)) return true;
            Bot.Sleep(Bot.Options.ActionDelay);
        }
        return !IsInProgress(id);
    }
    public void EnsureComplete(params int[] ids) { foreach (int id in ids) { EnsureComplete(id); Bot.Sleep(Bot.Options.ActionDelay); } }
    public bool UpdateQuest(int id)
    {
        if (EnsureLoad(id) is not { } quest) return false;
        UpdateQuest(quest.Value, quest.Slot);
        return true;
    }
    public void UpdateQuest(int value, int slot) => protocol.Call(new InjectCall($"{{\"t\":\"xt\",\"b\":{{\"r\":-1,\"o\":{{\"cmd\":\"updateQuest\",\"iValue\":{value},\"iIndex\":{slot}}}}}}}"));
    public void RegisterQuests(params int[] ids) => RegisterQuests(ids.Select(id => (id, -1)).ToArray());
    public void RegisterQuests(params (int questId, int rewardId)[] quests)
    {
        if (quests.Length == 0) return;
        lock (registered)
        {
            foreach (var (id, reward) in quests) registered[id] = reward;
            if (poller is null || poller.IsCompleted || pollingStop?.IsCancellationRequested == true)
            {
                pollingStop?.Dispose();
                pollingStop = CancellationTokenSource.CreateLinkedTokenSource(Bot.ScriptCts.Token);
                var token = pollingStop.Token;
                poller = Task.Run(() => Poll(token));
            }
        }
        foreach (var (_, reward) in quests) if (reward > 0) Bot.Drops.Add(reward);
        if (!Bot.Drops.Enabled) Bot.Drops.Start();
        NotifyRegistered();
    }
    public void UnregisterQuests(params int[] ids)
    {
        lock (registered) foreach (int id in ids) { registered.Remove(id); lastAccept.Remove(id); lastComplete.Remove(id); }
        NotifyRegistered();
    }
    public void UnregisterAllQuests()
    {
        Task? running;
        lock (registered)
        {
            registered.Clear(); lastAccept.Clear(); lastComplete.Clear();
            pollingStop?.Cancel();
            running = poller;
        }
        if (running is not null) try { running.Wait(TimeSpan.FromSeconds(2)); } catch (AggregateException) { }
        NotifyRegistered();
    }
    void NotifyRegistered()
    {
        PropertyChanged?.Invoke(this, new(nameof(Registered)));
        CommunityToolkit.Mvvm.Messaging.WeakReferenceMessenger.Default.Send(new CommunityToolkit.Mvvm.Messaging.Messages.PropertyChangedMessage<IEnumerable<int>>(this, nameof(Registered), Registered, Registered));
    }
    public void Reaccept(int id)
    {
        if (Bot.Lite.ReacceptQuest && TryGetQuest(id, out var quest) && !quest!.Once && IsAvailable(id)) Task.Run(() => Accept(id));
    }
    void Poll(CancellationToken token)
    {
        try
        {
            while (!token.IsCancellationRequested)
            {
                foreach (var (id, reward) in RegisteredRewards)
                {
                    token.ThrowIfCancellationRequested();
                    if (!Bot.Player.Playing) break;
                    long now = Environment.TickCount64;
                    lock (registered) if (!registered.ContainsKey(id) || lastComplete.TryGetValue(id, out long completed) && now - completed < 3000) continue;
                    Quest? quest = null;
                    for (int attempt = 0; attempt < 20 && !TryGetQuest(id, out quest); attempt++)
                    {
                        token.ThrowIfCancellationRequested();
                        Load(id);
                        Pause(1000, token);
                    }
                    if (quest is null) continue;
                    var drops = quest.Requirements.Concat(quest.AcceptRequirements).Where(i => !i.Temp).Select(i => i.ID).ToArray();
                    if (drops.Length > 0) Bot.Drops.Add(drops);
                    if (!IsInProgress(id))
                    {
                        lock (registered) if (lastAccept.TryGetValue(id, out long accepted) && now - accepted < 2000) continue;
                        Accept(id);
                        token.ThrowIfCancellationRequested();
                        lock (registered) lastAccept[id] = Environment.TickCount64;
                        Pause(Bot.Options.ActionDelay, token);
                    }
                    token.ThrowIfCancellationRequested();
                    if (!CanComplete(id)) continue;
                    int times = quest.Once || !string.IsNullOrEmpty(quest.Field) ? 1 : Bot.Flash.CallGameFunction<int>("world.maximumQuestTurnIns", id);
                    if (!Act(new CompleteCall(id, reward < 0 ? null : reward, times))) continue;
                    token.ThrowIfCancellationRequested();
                    lock (registered) lastComplete[id] = Environment.TickCount64;
                    Pause(Bot.Options.ActionDelay, token);
                    if (!Bot.Lite.ReacceptQuest)
                    {
                        Accept(id);
                        token.ThrowIfCancellationRequested();
                        lock (registered) lastAccept[id] = Environment.TickCount64;
                        Pause(Bot.Options.ActionDelay, token);
                    }
                    break;
                }
                Pause(RegisterCompleteInterval, token);
            }
        }
        catch (OperationCanceledException) { }
        catch (InvalidOperationException) { }
    }
    static void Pause(int ms, CancellationToken token) { if (token.WaitHandle.WaitOne(ms)) token.ThrowIfCancellationRequested(); }
    public void LoadCachedQuests()
    {
        if (cacheLoaded) return;
        var path = ClientFileSources.SkuaQuestsFile;
        Cached = Newtonsoft.Json.JsonConvert.DeserializeObject<List<QuestData>>(File.ReadAllText(path)) ?? [];
        cacheLoaded = true;
    }
    public List<QuestData> GetCachedQuests(int start, int count) { LoadCachedQuests(); return CachedDictionary.Values.Skip(start).Take(count).ToList(); }
    public List<QuestData> GetCachedQuests(params int[] ids) { LoadCachedQuests(); return ids.Where(CachedDictionary.ContainsKey).Select(id => CachedDictionary[id]).ToList(); }
}
