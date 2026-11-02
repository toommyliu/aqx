using Skua.Core.Interfaces;

namespace Aqx.Skua;

public sealed partial class Shim
{
    readonly Dictionary<string, HashSet<string>> tags = new(StringComparer.OrdinalIgnoreCase);

    void Accounts()
    {
        On("IScriptAccounts.GetTags()", _ => Tags(Me["name"]?.ToString()).ToList());
        On("IScriptAccounts.GetTags(String)", args => Tags((string?)args[0]).ToList());
        On("IScriptAccounts.HasTag(String)", args => Tags(Me["name"]?.ToString()).Contains((string)args[0]!));
        On("IScriptAccounts.HasTag(String, String)", args => Tags((string?)args[0]).Contains((string)args[1]!));
        On("IScriptAccounts.AddTag(String)", args => Tags(Me["name"]?.ToString()).Add((string)args[0]!));
        On("IScriptAccounts.AddTag(String, String)", args => Tags((string?)args[0]).Add((string)args[1]!));
        On("IScriptAccounts.RemoveTag(String)", args => Tags(Me["name"]?.ToString()).Remove((string)args[0]!));
        On("IScriptAccounts.RemoveTag(String, String)", args => Tags((string?)args[0]).Remove((string)args[1]!));
        On("IScriptAccounts.AddTags(String[])", args => AddTags(Me["name"]?.ToString(), (string[])args[0]!));
        On("IScriptAccounts.AddTags(String, String[])", args => AddTags((string?)args[0], (string[])args[1]!));
        On("IScriptAccounts.RemoveTags(String[])", args => RemoveTags(Me["name"]?.ToString(), (string[])args[0]!));
        On("IScriptAccounts.RemoveTags(String, String[])", args => RemoveTags((string?)args[0], (string[])args[1]!));
        On("IScriptAccounts.SetTags(String[])", args => SetTags(Me["name"]?.ToString(), (string[])args[0]!));
        On("IScriptAccounts.SetTags(String, String[])", args => SetTags((string?)args[0], (string[])args[1]!));
        On("IScriptAccounts.ClearTags()", _ => SetTags(Me["name"]?.ToString(), []));
        On("IScriptAccounts.ClearTags(String)", args => SetTags((string?)args[0], []));
        On("IScriptAccounts.GetAllAccounts()", _ => tags.Select(t => new ManagedAccount(t.Key, "", t.Key, [.. t.Value])).ToList());
    }

    HashSet<string> Tags(string? username)
    {
        lock (tags)
        {
            string name = username ?? "";
            if (!tags.TryGetValue(name, out HashSet<string>? found))
                tags[name] = found = new(StringComparer.OrdinalIgnoreCase);
            return found;
        }
    }

    bool AddTags(string? username, string[] added)
    {
        Tags(username).UnionWith(added);
        return true;
    }

    bool RemoveTags(string? username, string[] removed)
    {
        Tags(username).ExceptWith(removed);
        return true;
    }

    bool SetTags(string? username, string[] replacing)
    {
        HashSet<string> current = Tags(username);
        current.Clear();
        current.UnionWith(replacing);
        return true;
    }
}
