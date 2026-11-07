namespace Mira.Skua;

public sealed class MiraGetScriptsService : IGetScriptsService
{
    public RangedObservableCollection<ScriptInfo> Scripts { get; } = new();
    public event PropertyChangedEventHandler? PropertyChanged;
    public async Task<bool> UpdateSkillSetsFile()
    {
        try
        {
            string content = await ValidatedHttpExtensions.GetStringAsync(HttpClients.GitHubRaw, "auqw/Scripts/refs/heads/Skua/Skills/AdvancedSkills.json");
            await File.WriteAllTextAsync(ClientFileSources.SkuaAdvancedSkillsFile, content);
            return true;
        }
        catch { return false; }
    }
    [Unsupported] public ValueTask<List<ScriptInfo>> GetScriptsAsync(IProgress<string>? progress, CancellationToken token) => throw new NotSupportedException("IGetScriptsService.GetScriptsAsync");
    [Unsupported] public Task RefreshScriptsAsync(IProgress<string>? progress, CancellationToken token) => throw new NotSupportedException("IGetScriptsService.RefreshScriptsAsync");
    [Unsupported] public Task<int> IncrementalUpdateScriptsAsync(IProgress<string>? progress, CancellationToken token) => throw new NotSupportedException("IGetScriptsService.IncrementalUpdateScriptsAsync");
    [Unsupported] public Task<long> CheckAdvanceSkillSetsUpdates() => throw new NotSupportedException("IGetScriptsService.CheckAdvanceSkillSetsUpdates");
    [Unsupported] public Task DownloadScriptAsync(ScriptInfo info) => throw new NotSupportedException("IGetScriptsService.DownloadScriptAsync");
    [Unsupported] public Task<int> DownloadAllWhereAsync(Func<ScriptInfo, bool> pred) => throw new NotSupportedException("IGetScriptsService.DownloadAllWhereAsync");
    [Unsupported] public Task DeleteScriptAsync(ScriptInfo info) => throw new NotSupportedException("IGetScriptsService.DeleteScriptAsync");
    [Unsupported] public Task<bool> UpdateQuestDataFile() => throw new NotSupportedException("IGetScriptsService.UpdateQuestDataFile");
    [Unsupported] public Task<long> CheckJunkItemsUpdates() => throw new NotSupportedException("IGetScriptsService.CheckJunkItemsUpdates");
    [Unsupported] public Task<bool> UpdateJunkItemsFile() => throw new NotSupportedException("IGetScriptsService.UpdateJunkItemsFile");
}
