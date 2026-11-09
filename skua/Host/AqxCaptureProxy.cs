namespace Aqx.Skua;

public sealed class AqxCaptureProxy : ICaptureProxy
{
    [Unsupported] public IPEndPoint? Destination { get => throw new NotSupportedException("ICaptureProxy.Destination"); set => throw new NotSupportedException("ICaptureProxy.Destination"); }
    [Unsupported] public List<IInterceptor> Interceptors { get => throw new NotSupportedException("ICaptureProxy.Interceptors"); }
    [Unsupported] public bool Running { get => throw new NotSupportedException("ICaptureProxy.Running"); }
    [Unsupported] public void Start()  => throw new NotSupportedException("ICaptureProxy.Start");
    [Unsupported] public void Stop()  => throw new NotSupportedException("ICaptureProxy.Stop");
    [Unsupported] public void Dispose() => throw new NotSupportedException("AqxCaptureProxy.Dispose");
    [Unsupported] public event System.ComponentModel.PropertyChangedEventHandler? PropertyChanged { add => throw new NotSupportedException("AqxCaptureProxy.PropertyChanged"); remove => throw new NotSupportedException("AqxCaptureProxy.PropertyChanged"); }
}
