namespace Skua.Core.Flash;

[AttributeUsage(AttributeTargets.Method, Inherited = false, AllowMultiple = false)]
sealed class MethodCallBindingAttribute(string name) : Attribute
{
    public string Name { get; set; } = name;
    public string? Default { get; set; }
}
