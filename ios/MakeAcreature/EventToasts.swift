import Foundation

/// Milestone / evolution toasts (`tank.h` event kinds). Phase 6 polish:
/// pause-on-event is a setting; the strings are the player-facing copy.
enum EventToast {
    /// Once-per-tank milestones (and a few raw kinds that deserve a sting).
    static func message(kind: UInt32) -> String? {
        switch kind {
        case 32: return "FIRST LIFE"
        case 33: return "FIRST DIVISION"
        case 34: return "FIRST DEATH"
        case 35: return "NEW COLOUR"
        case 36: return "MULTICELLULAR"
        case 37: return "GIANT CELL"
        case 38: return "FUNGUS"
        case 39: return "HETEROTROPH"
        case 40: return "PREDATION"
        case 41: return "POISON"
        case 42: return "SIGNALLING"
        case 43: return "SOMETHING MOVED"
        case 44: return "NEW SPECIES"
        case 45: return "WARM-BLOODED"
        case 46: return "LIFE ON LAND"
        case 47: return "CUTTING TOOK ROOT"
        case 48: return "COLOUR CROWN"
        case 49: return "AN EYE"
        case 50: return "LOCOMOTION"
        case 51: return "ESCAPE"
        case 52: return "A MOUTH"
        // Repeatable / ambient.
        case 10: return "TANK LIFELESS"
        case 11: return "GLASS NEEDS CLEANING"
        case 24: return "WATER IS CLOUDY"
        default: return nil
        }
    }

    /// Kinds that pause the sim when `pauseOnEvent` is on (big evolution beats).
    static func shouldPause(kind: UInt32) -> Bool {
        switch kind {
        case 36, 39, 40, 43, 49, 50, 51, 52: return true
        default: return false
        }
    }

    /// Sound-sting hook for Phase 6 audio (MOVED / PREDATION / ESCAPE raw + MS).
    static func soundTrigger(kind: UInt32) -> String? {
        switch kind {
        case 21, 43: return "moved"
        case 16, 40: return "predation"
        case 30, 51: return "escape"
        case 49: return "eye"
        case 50: return "locomotion"
        case 52: return "chamber"
        default: return nil
        }
    }
}
