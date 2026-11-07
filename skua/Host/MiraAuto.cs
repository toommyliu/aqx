namespace Mira.Skua;

public sealed class MiraAuto : IScriptAuto
{
    [Unsupported] public bool IsRunning { get => throw new NotSupportedException("IScriptAuto.IsRunning"); }
    [Unsupported] public void StartAutoAttack(string? className = null, ClassUseMode classUseMode = ClassUseMode.Base, int[]? manualMapIDs = null)  => throw new NotSupportedException("IScriptAuto.StartAutoAttack");
    [Unsupported] public void StartAutoHunt(string? className = null, ClassUseMode classUseMode = ClassUseMode.Base, int[]? manualMapIDs = null)  => throw new NotSupportedException("IScriptAuto.StartAutoHunt");
    [Unsupported] public void Stop()  => throw new NotSupportedException("IScriptAuto.Stop");
    [Unsupported] public ValueTask StopAsync()  => throw new NotSupportedException("IScriptAuto.StopAsync");
    [Unsupported] public event System.ComponentModel.PropertyChangedEventHandler? PropertyChanged { add => throw new NotSupportedException("MiraAuto.PropertyChanged"); remove => throw new NotSupportedException("MiraAuto.PropertyChanged"); }
}
