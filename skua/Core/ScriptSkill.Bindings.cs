namespace Skua.Core.Scripts;

public partial class ScriptSkill
{
    public bool CanUseSkill(int index)
    {
        try
        {
            return Flash.Call<bool>("canUseSkill", index);
        }
        catch
        {
            return false;
        }
    }

    public bool UseSkill(int index)
    {
        try
        {
            return Flash.Call<bool>("useSkill", index);
        }
        catch
        {
            return false;
        }
    }
}
