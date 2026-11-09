namespace Aqx.Skua;

public sealed class AqxAccounts(State state) : IScriptAccounts
{
    readonly Dictionary<string, AccountData> accounts = new(AqxSettings.Read<Dictionary<string, AccountData>>("ManagedAccounts") ?? [], StringComparer.OrdinalIgnoreCase);
    string Username => state.Me?.Name ?? "";
    List<string> Tags(string username) => accounts.GetValueOrDefault(username)?.Tags ?? [];
    bool Change(string username, Func<List<string>, bool> change)
    {
        lock (accounts)
        {
            if (!accounts.TryGetValue(username, out var account) || !change(account.Tags)) return false;
            AqxSettings.Write("ManagedAccounts", accounts);
            return true;
        }
    }
    public List<string> GetTags() => GetTags(Username);
    public List<string> GetTags(string username) { lock (accounts) return [.. Tags(username)]; }
    public bool HasTag(string tag) => HasTag(Username, tag);
    public bool HasTag(string username, string tag) { lock (accounts) return Tags(username).Contains(tag, StringComparer.OrdinalIgnoreCase); }
    public bool AddTag(string tag) => AddTag(Username, tag);
    public bool AddTag(string username, string tag) => AddTags(username, [tag]);
    public bool RemoveTag(string tag) => RemoveTag(Username, tag);
    public bool RemoveTag(string username, string tag) => RemoveTags(username, [tag]);
    public void AddTags(params string[] added) => AddTags(Username, added);
    public bool AddTags(string username, params string[] added) => Change(username, tags => { bool changed = false; foreach (var tag in added) if (!tags.Contains(tag, StringComparer.OrdinalIgnoreCase)) { tags.Add(tag); changed = true; } return changed; });
    public void RemoveTags(params string[] removed) => RemoveTags(Username, removed);
    public bool RemoveTags(string username, params string[] removed) => Change(username, tags => { bool changed = false; foreach (var tag in removed) { int index = tags.FindIndex(t => t.Equals(tag, StringComparison.OrdinalIgnoreCase)); if (index >= 0) { tags.RemoveAt(index); changed = true; } } return changed; });
    public void SetTags(params string[] replacing) => SetTags(Username, replacing);
    public bool SetTags(string username, params string[] replacing) => Change(username, tags => { tags.Clear(); tags.AddRange(replacing); return true; });
    public void ClearTags() => SetTags([]);
    public bool ClearTags(string username) => SetTags(username, []);
    public List<ManagedAccount> GetAllAccounts() { lock (accounts) return accounts.Select(t => new ManagedAccount(t.Key, t.Value.Password, t.Value.DisplayName, [.. t.Value.Tags])).ToList(); }
}
