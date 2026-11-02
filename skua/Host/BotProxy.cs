using System.Collections.Concurrent;
using System.Reflection;
using System.Reflection.Emit;
using Skua.Core.Interfaces;

namespace Aqx.Skua;

public class BotProxy : DispatchProxy
{
    static readonly ConcurrentDictionary<MethodInfo, Func<object, object?[], object?>> Defaults = new();

    string path = "Bot";
    Type type = typeof(IScriptInterface);
    Shim shim = null!;
    readonly ConcurrentDictionary<string, object> children = new();

    public static IScriptInterface Create(Shim shim) => (IScriptInterface)Make(typeof(IScriptInterface), "Bot", shim);

    public static object Service(Type type, Shim shim) => Make(type, $"Services.{type.Name}", shim);

    static object Make(Type type, string path, Shim shim)
    {
        object proxy = Create(type, typeof(BotProxy));
        ((BotProxy)proxy).path = path;
        ((BotProxy)proxy).type = type;
        ((BotProxy)proxy).shim = shim;
        return proxy;
    }

    protected override object? Invoke(MethodInfo? method, object?[]? args)
    {
        MethodInfo called = method!;
        if (shim.TryGet(type, called, out Func<object?[], MethodInfo, object?>? handler))
            return handler(args ?? [], called);
        if (!called.IsAbstract)
            return Defaults.GetOrAdd(called, DefaultBody)(this, args ?? []);

        string member = Describe(called);
        if (called.IsSpecialName && called.Name.StartsWith("get_") && called.GetParameters().Length == 0 && IsSkuaInterface(called.ReturnType))
            return children.GetOrAdd(called.Name, _ => Make(called.ReturnType, $"{path}.{member}", shim));
        throw new NotImplementedException($"{path}.{member} is not implemented");
    }

    public static Func<object, object?[], object?> DefaultBody(MethodInfo method)
    {
        ParameterInfo[] parameters = method.GetParameters();
        DynamicMethod body = new($"default_{method.Name}", typeof(object), [typeof(object), typeof(object?[])], typeof(BotProxy).Module, skipVisibility: true);
        ILGenerator il = body.GetILGenerator();
        LocalBuilder?[] byRef = new LocalBuilder?[parameters.Length];

        il.Emit(OpCodes.Ldarg_0);
        il.Emit(OpCodes.Castclass, method.DeclaringType!);
        for (int i = 0; i < parameters.Length; i++)
        {
            Type type = parameters[i].ParameterType;
            Type value = type.IsByRef ? type.GetElementType()! : type;
            if (!parameters[i].IsOut)
            {
                il.Emit(OpCodes.Ldarg_1);
                il.Emit(OpCodes.Ldc_I4, i);
                il.Emit(OpCodes.Ldelem_Ref);
                il.Emit(OpCodes.Unbox_Any, value);
            }
            if (type.IsByRef)
            {
                byRef[i] = il.DeclareLocal(value);
                if (!parameters[i].IsOut)
                    il.Emit(OpCodes.Stloc, byRef[i]!);
                il.Emit(OpCodes.Ldloca, byRef[i]!);
            }
        }
        il.Emit(OpCodes.Call, method);

        LocalBuilder? result = method.ReturnType == typeof(void) ? null : il.DeclareLocal(method.ReturnType);
        if (result is not null)
            il.Emit(OpCodes.Stloc, result);
        for (int i = 0; i < parameters.Length; i++)
        {
            if (byRef[i] is not { } local)
                continue;
            il.Emit(OpCodes.Ldarg_1);
            il.Emit(OpCodes.Ldc_I4, i);
            il.Emit(OpCodes.Ldloc, local);
            il.Emit(OpCodes.Box, local.LocalType);
            il.Emit(OpCodes.Stelem_Ref);
        }
        if (result is null)
            il.Emit(OpCodes.Ldnull);
        else
        {
            il.Emit(OpCodes.Ldloc, result);
            il.Emit(OpCodes.Box, result.LocalType);
        }
        il.Emit(OpCodes.Ret);
        return body.CreateDelegate<Func<object, object?[], object?>>();
    }

    static bool IsSkuaInterface(Type type) => type.IsInterface && type.Namespace?.StartsWith("Skua.") == true;

    static string Describe(MethodInfo method)
    {
        string name = method.Name;
        if (method.IsSpecialName)
        {
            int split = name.IndexOf('_');
            return (name[..split], name[(split + 1)..]) switch
            {
                ("get", var property) => property,
                ("set", var property) => $"{property} (set)",
                ("add", var @event) => $"{@event} (+=)",
                ("remove", var @event) => $"{@event} (-=)",
                _ => name,
            };
        }
        return $"{name}({string.Join(", ", method.GetParameters().Select(p => p.ParameterType.Name))})";
    }
}
