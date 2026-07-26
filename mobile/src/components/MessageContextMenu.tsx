// Long-press context menu for chat messages.
// Items: Revert, Edit, Copy.
// Revert truncates the conversation to the selected message.
// Edit re-runs the assistant turn from an edited user prompt.
// Copy copies the message text to the clipboard.

import { useMemo } from 'react';
import {
  Modal,
  Pressable,
  StyleSheet,
  Text,
  useWindowDimensions,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

type MenuPosition = { x: number; y: number };

interface MessageContextMenuProps {
  visible: boolean;
  position: MenuPosition;
  canEdit: boolean;
  canRevert: boolean;
  onClose: () => void;
  onCopy: () => void;
  onEdit: () => void;
  onRevert: () => void;
}

type MenuItem = {
  label: string;
  icon: keyof typeof Ionicons.glyphMap;
  onPress: () => void;
  disabled?: boolean;
};

const MENU_WIDTH = 160;
const MENU_ITEM_HEIGHT = 48;
const MENU_VERTICAL_PADDING = 8;

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    overlay: {
      flex: 1,
      backgroundColor: colors.overlay,
    },
    menu: {
      position: 'absolute',
      width: MENU_WIDTH,
      borderRadius: 12,
      paddingVertical: MENU_VERTICAL_PADDING,
      shadowColor: colors.shadow,
      shadowOffset: { width: 0, height: 4 },
      shadowOpacity: 0.3,
      shadowRadius: 8,
      elevation: 8,
    },
    menuItem: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      paddingHorizontal: 16,
      height: MENU_ITEM_HEIGHT,
    },
    menuItemPressed: {
      backgroundColor: colors.brandPressed,
    },
    menuItemDisabled: {
      opacity: 0.5,
    },
    menuItemText: {
      fontSize: 15,
    },
  });

export function MessageContextMenu({
  visible,
  position,
  canEdit,
  canRevert,
  onClose,
  onCopy,
  onEdit,
  onRevert,
}: MessageContextMenuProps) {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const { width: screenWidth, height: screenHeight } = useWindowDimensions();

  const menuHeight = MENU_VERTICAL_PADDING * 2 + MENU_ITEM_HEIGHT * 3;

  const adjustedPosition = useMemo(() => {
    let x = position.x - MENU_WIDTH / 2;
    let y = position.y;
    if (x < 8) x = 8;
    if (x + MENU_WIDTH > screenWidth - 8) x = screenWidth - MENU_WIDTH - 8;
    if (y + menuHeight > screenHeight - 8) {
      y = screenHeight - menuHeight - 8;
    }
    return { x, y };
  }, [position, screenWidth, screenHeight, menuHeight]);

  const items: MenuItem[] = [
    {
      label: '元に戻す',
      icon: 'arrow-undo-outline',
      onPress: onRevert,
      disabled: !canRevert,
    },
    {
      label: '編集',
      icon: 'pencil-outline',
      onPress: onEdit,
      disabled: !canEdit,
    },
    { label: 'コピー', icon: 'copy-outline', onPress: onCopy },
  ];

  return (
    <Modal
      visible={visible}
      transparent
      animationType="fade"
      onRequestClose={onClose}
    >
      <Pressable style={styles.overlay} onPress={onClose}>
        <View
          style={[
            styles.menu,
            {
              left: adjustedPosition.x,
              top: adjustedPosition.y,
              backgroundColor: colors.white,
              minHeight: menuHeight,
            },
          ]}
          onStartShouldSetResponder={() => true}
        >
          {items.map((item) => (
            <Pressable
              key={item.label}
              style={({ pressed }) => [
                styles.menuItem,
                pressed && styles.menuItemPressed,
                item.disabled && styles.menuItemDisabled,
              ]}
              disabled={item.disabled}
              onPress={() => {
                haptic.light();
                onClose();
                item.onPress();
              }}
            >
              <Ionicons
                name={item.icon}
                size={18}
                color={item.disabled ? colors.gray : colors.brand}
              />
              <Text
                style={[
                  styles.menuItemText,
                  { color: item.disabled ? colors.gray : colors.black },
                ]}
              >
                {item.label}
              </Text>
            </Pressable>
          ))}
        </View>
      </Pressable>
    </Modal>
  );
}
