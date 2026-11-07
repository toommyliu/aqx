namespace Mira.Skua;

public sealed class MiraAuraMonitorService : IAuraMonitorService
{
    [Unsupported] public bool IsMonitoring { get => throw new NotSupportedException("IAuraMonitorService.IsMonitoring"); }
    [Unsupported] public int SubscriberCount { get => throw new NotSupportedException("IAuraMonitorService.SubscriberCount"); }
    [Unsupported] public event Action<string, DateTimeOffset, float, float, SubjectType>? AuraActivated { add => throw new NotSupportedException("IAuraMonitorService.AuraActivated"); remove => throw new NotSupportedException("IAuraMonitorService.AuraActivated"); }
    [Unsupported] public event Action<string, SubjectType>? AuraDeactivated { add => throw new NotSupportedException("IAuraMonitorService.AuraDeactivated"); remove => throw new NotSupportedException("IAuraMonitorService.AuraDeactivated"); }
    [Unsupported] public event Action<string, float, float, SubjectType>? AuraStackChanged { add => throw new NotSupportedException("IAuraMonitorService.AuraStackChanged"); remove => throw new NotSupportedException("IAuraMonitorService.AuraStackChanged"); }
    [Unsupported] public void EnsureMonitoring(int pollIntervalMs = 100)  => throw new NotSupportedException("IAuraMonitorService.EnsureMonitoring");
    [Unsupported] public void StartMonitoring(int pollIntervalMs = 100)  => throw new NotSupportedException("IAuraMonitorService.StartMonitoring");
    [Unsupported] public void StopMonitoring()  => throw new NotSupportedException("IAuraMonitorService.StopMonitoring");
    [Unsupported] public void Dispose() => throw new NotSupportedException("IAuraMonitorService.Dispose");
}
