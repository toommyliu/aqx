namespace Mira.Skua;

static class MiraAuras
{
    public static Aura ToScript(AuraState aura) => new()
    {
        Name = aura.Name, Value = (float)(aura.Value ?? 1), Duration = (int)aura.Duration,
        UnixTimeStamp = aura.Expires is { } expiry ? (long)expiry - (long)(aura.Duration * 1000) : 0,
        Category = aura.Category ?? ""
    };
    public static AuraSnapshot Snapshot(AuraState aura) => new()
    {
        Name = aura.Name, Icon = aura.Icon, Description = aura.Description, Stacks = aura.Stacks, Duration = aura.Duration, Persistent = aura.Persist,
        RemainingTime = aura.Expires is { } expiry ? Math.Max(0, ((long)expiry - DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()) / 1000.0) : 0
    };
}
