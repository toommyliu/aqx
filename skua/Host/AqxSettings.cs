namespace Aqx.Skua;

static class AqxSettings
{
    static readonly global::Skua.Core.Services.UnifiedSettingsService settings = Create();
    static global::Skua.Core.Services.UnifiedSettingsService Create()
    {
        var service = new global::Skua.Core.Services.UnifiedSettingsService();
        service.Initialize(AppRole.Client);
        return service;
    }
    public static T? Read<T>(string name) => settings.Get<T>(name);
    public static void Write<T>(string name, T value) => settings.Set(name, value);
}
